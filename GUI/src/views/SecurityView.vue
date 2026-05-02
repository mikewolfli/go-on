<template>
  <el-space direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>{{ t("security.title") }}</template>

      <el-space direction="vertical" fill style="width: 100%">
        <el-text>{{ t("security.hint") }}</el-text>

        <!-- 安全评分 -->
        <el-card shadow="hover">
          <template #header>
            <span>{{ t("security.securityScore") }}</span>
          </template>
          <el-space direction="vertical" fill style="width: 100%">
            <el-space>
              <el-tag :type="governanceState === 'healthy' ? 'success' : 'warning'">
                {{ t("security.governanceLabel") }}: {{ governanceState }}
              </el-tag>
              <el-tag type="info">{{ t("security.rulesLabel") }}: {{ rulesVersion }}</el-tag>
              <el-tag type="info">
                {{ t("security.dynamicRules") }}: {{ dynamicRulesCount }}
              </el-tag>
              <el-tag type="info">
                {{ t("security.auditRecent") }}: {{ auditRecentCount }}
              </el-tag>
              <el-tag :type="strictEnabled ? 'success' : 'danger'">
                {{ t("security.productionStrictLabel") }}: {{ strictEnabled ? t("security.on") : t("security.off") }}
              </el-tag>
              <el-tag :type="entryAuthEnabled ? 'success' : 'warning'">
                {{ t("security.entryAuthLabel") }}: {{ entryAuthEnabled ? t("security.on") : t("security.off") }}
              </el-tag>
              <el-tag type="info">
                {{ t("security.entryRateLimitLabel") }}: {{ t("security.entryRateLimitValue", { rpm: entryRateLimitRpm, burst: entryRateLimitBurst }) }}
              </el-tag>
              <el-tag :type="providerDegradedCount > 0 ? 'warning' : 'success'">
                {{ t("security.providerReadyLabel") }}: {{ providerReadyCount }}/{{ providerConfiguredCount }}
              </el-tag>
              <el-tag :type="releaseBlockedGateCount > 0 ? 'warning' : 'success'">
                {{ releaseReadinessDisplay }}
              </el-tag>
              <el-tag type="info">
                schema: g={{ governanceSchemaVersion }} / r={{ readinessSchemaVersion }}
              </el-tag>
            </el-space>
            <el-row :gutter="16">
              <el-col :span="6">
                <!-- el-statistic is deprecated in Element Plus but still works -->
                <el-statistic :title="t('security.overallScore')" :value="overallScore" suffix="/100" />
              </el-col>
              <el-col :span="6">
                <!-- el-statistic is deprecated in Element Plus but still works -->
                <el-statistic :title="t('security.credsScore')" :value="credsScore" suffix="/100" />
              </el-col>
              <el-col :span="6">
                <!-- el-statistic is deprecated in Element Plus but still works -->
                <el-statistic :title="t('security.auditScore')" :value="auditScore" suffix="/100" />
              </el-col>
              <el-col :span="6">
                <!-- el-statistic is deprecated in Element Plus but still works -->
                <el-statistic :title="t('security.configScore')" :value="configScore" suffix="/100" />
              </el-col>
            </el-row>
            <el-progress :percentage="overallScore" color="hsl(120, 100%, 40%)" />
          </el-space>
        </el-card>

        <!-- 敏感字段管理 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("security.sensitiveFields") }}</span>
              <el-button size="small" @click="auditSensitiveFields"> {{ t("security.audit") }}</el-button>
            </div>
          </template>
          <el-table :data="sensitiveFields" border stripe>
            <el-table-column prop="name" :label="t('common.name')" width="150" />
            <el-table-column prop="location" :label="t('security.location')" width="200" />
            <el-table-column prop="status" :label="t('common.status')" width="100">
              <template #default="{ row }">
                <el-tag :type="row.status === 'masked' ? 'success' : 'warning'">
                  {{ row.status }}
                </el-tag>
              </template>
            </el-table-column>
            <el-table-column prop="value" :label="t('security.maskedValue')" width="200" />
            <el-table-column :label="t('common.action')" width="100">
              <template #default="{ row }">
                <el-popover trigger="hover" :title="t('security.revealWarning')">
                  <template #default>
                    <el-button size="small" type="warning">{{ row.actual_length }} chars</el-button>
                  </template>
                  <template #reference>
                    <el-button size="small" type="text">{{ t("security.reveal") }}</el-button>
                  </template>
                </el-popover>
              </template>
            </el-table-column>
          </el-table>
        </el-card>

        <!-- 凭据风险 -->
        <el-card shadow="hover">
          <template #header>
            <span>{{ t("security.risks") }}</span>
          </template>
          <el-alert
            v-for="(risk, idx) in risks"
            :key="idx"
            :title="risk.title"
            :description="risk.description"
            :type="risk.type"
            closable
            style="margin-bottom: 8px"
          >
            <template #default v-if="risk.action">
              <el-button type="text" size="small" @click="fixRisk(risk.id)">{{ t("security.fix") }}</el-button>
            </template>
          </el-alert>
        </el-card>

        <!-- 审计日志 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("security.auditLog") }}</span>
              <el-button size="small" @click="exportAuditLog">{{ t("security.export") }}</el-button>
            </div>
          </template>
          <el-table :data="auditLogs" border stripe max-height="300">
            <el-table-column prop="timestamp" :label="t('common.time')" width="180" />
            <el-table-column prop="action" :label="t('security.action')" width="150" />
            <el-table-column prop="resource" :label="t('security.resource')" width="150" />
            <el-table-column prop="user" :label="t('security.user')" width="100" />
            <el-table-column prop="result" :label="t('common.result')" width="100">
              <template #default="{ row }">
                <el-tag :type="row.result === 'success' ? 'success' : 'danger'">
                  {{ row.result }}
                </el-tag>
              </template>
            </el-table-column>
          </el-table>
        </el-card>

        <!-- 安全建议 -->
        <el-card shadow="hover" v-if="recommendations.length > 0">
          <template #header>
            <span>{{ t("security.recommendations") }}</span>
          </template>
          <el-steps :active="0" process-status="finish">
            <el-step v-for="(rec, idx) in recommendations" :key="idx" :title="rec.title">
              <template #description>
                <el-text>{{ rec.description }}</el-text>
              </template>
            </el-step>
          </el-steps>
        </el-card>

        <el-divider />

        <!-- 更新设置 -->
        <el-space>
          <el-checkbox v-model="autoMaskSensitive">{{ t("security.autoMask") }}</el-checkbox>
          <el-checkbox v-model="auditEnabled">{{ t("security.enableAudit") }}</el-checkbox>
          <el-button type="primary" @click="saveSecurity">{{ t("common.save") }}</el-button>
        </el-space>
      </el-space>
    </el-card>
  </el-space>
</template>

<script setup lang="ts">
import { onMounted, ref, reactive, computed } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { useI18n } from "vue-i18n";
import {
  getGovernanceAuditRecent,
  getGovernanceStatus,
  getProviderStatus,
  getReleaseReadiness,
  remediateRisk,
  saveGovernanceConfig,
  type GovernanceAuditEvent,
} from "../services/rpcService";
import { normalizeErrorMessage } from "../utils/errors";

const { t } = useI18n();

// Score penalty constants
const MAX_GOVERNANCE_PENALTY = 45;
const PENALTY_PUA_FAILED = 8;
const PENALTY_BREAKER_OPEN = 10;
const PENALTY_WARNING = 4;
const PENALTY_STRICT_VIOLATION = 12;
const PENALTY_PROVIDER_DEGRADED = 6;
const PENALTY_RELEASE_BLOCKED_GATE = 8;
const PENALTY_STRICT_DISABLED = 6;
const PENALTY_ENTRY_AUTH_DISABLED = 8;
const PENALTY_ENTRY_AUTH_KEY_MISSING = 10;
const MIN_GOVERNANCE_SCORE = 40;
const BASE_HEALTHY_SCORE = 100;
const BASE_UNHEALTHY_SCORE = 65;
const MIN_CREDS_SCORE = 40;
const PENALTY_CREDS_WARNING = 8;
const PENALTY_CREDS_VIOLATION = 10;
const BASE_CREDS_SCORE = 90;
const MIN_AUDIT_SCORE = 35;
const PENALTY_AUDIT_PUA = 10;
const PENALTY_AUDIT_BREAKER = 6;
const BASE_AUDIT_SCORE = 95;
const MIN_CONFIG_SCORE = 35;
const PENALTY_CONFIG_WARNING = 9;
const PENALTY_CONFIG_VIOLATION = 12;
const PENALTY_CONFIG_STRICT_DISABLED = 8;
const BASE_CONFIG_SCORE = 95;

const overallScore = ref(65);
const credsScore = ref(60);
const auditScore = ref(60);
const configScore = ref(60);

const autoMaskSensitive = ref(true);
const auditEnabled = ref(true);
const governanceState = ref("unknown");
const rulesVersion = ref("-");
const strictEnabled = ref(false);
const entryAuthEnabled = ref(false);
const entryAuthKeyConfigured = ref(false);
const entryRateLimitRpm = ref(0);
const entryRateLimitBurst = ref(0);
const providerReadyCount = ref(0);
const providerDegradedCount = ref(0);
const providerConfiguredCount = ref(0);
const releaseReadinessStatus = ref("unknown");
const releaseBlockedGateCount = ref(0);
const blockedGateNamesText = ref("");
const multiUserMode = ref("single_user");
const multiUserReady = ref(false);
const multiUserLifecycleReady = ref(false);
const dualTrackConsistencyReady = ref(false);
const multiUserSource = ref("default");
const governanceSchemaVersion = ref("unknown");
const readinessSchemaVersion = ref("unknown");
const dynamicRulesCount = ref(0);
const auditRecentCount = ref(0);

const sensitiveFields = ref<Array<{ name: string; location: string; status: string; value: string; actual_length: number }>>([]);

const risks = reactive<Array<{ id: string; type: "warning" | "info" | "success"; title: string; description: string; action: boolean }>>([]);

const auditLogs = ref<Array<{ timestamp: string; action: string; resource: string; user: string; result: string }>>([]);

const releaseReadinessDisplay = computed(() => {
  return `${releaseReadinessStatus.value} (${multiUserMode.value} / ${multiUserReady.value ? "ready" : "blocked"} / lifecycle=${multiUserLifecycleReady.value ? "ready" : "blocked"} / consistency=${dualTrackConsistencyReady.value ? "ready" : "blocked"} / ${multiUserSource.value} / ${blockedGateNamesText.value || "-"})`;
});

const recommendations = ref([
  {
    title: t("security.rec1Title"),
    description: t("security.rec1Desc"),
  },
  {
    title: t("security.rec2Title"),
    description: t("security.rec2Desc"),
  },
  {
    title: t("security.rec3Title"),
    description: t("security.rec3Desc"),
  },
]);

function normalizeLevelTag(level: string): "success" | "warning" {
  return level.toLowerCase() === "healthy" ? "success" : "warning";
}

function normalizeAuditResultTag(result: string): string {
  const lower = String(result || "").toLowerCase();
  return lower === "success" || lower === "ok" ? "success" : "danger";
}

async function refreshGovernanceAuditRecent(limit = 20) {
  try {
    const parsed = await getGovernanceAuditRecent(limit);
    const events = Array.isArray(parsed?.audit?.events) ? parsed.audit.events : [];
    auditRecentCount.value = events.length;

    auditLogs.value = events.map((event: GovernanceAuditEvent) => ({
      timestamp: new Date(Number(event.timestamp || 0) * 1000).toLocaleString(),
      action: String(event.action || "unknown"),
      resource: String(event.detail?.escalation_level || "governance"),
      user: String(event.actor || "system"),
      result: normalizeAuditResultTag(String(event.result || "warning")),
    }));
  } catch (error) {
    ElMessage.error(t("security.auditRecentFailed", { error: normalizeErrorMessage(error) }));
  }
}

async function refreshGovernanceStatus() {
  try {
    const parsed = await getGovernanceStatus();
    const governance = parsed?.governance || {};

    governanceState.value = String(governance.status || "unknown");
    governanceSchemaVersion.value = String(
      governance.schema_version || governance.artifact_contract?.schema_version || "unknown",
    );
    rulesVersion.value = String(governance.rules?.version || "-");
    strictEnabled.value = governance.config?.production_strict === true;
    entryAuthEnabled.value = governance.config?.entry_auth_enabled === true;
    entryAuthKeyConfigured.value = governance.config?.entry_auth_key_configured === true;
    entryRateLimitRpm.value = Number(governance.config?.entry_rate_limit_rpm || 0);
    entryRateLimitBurst.value = Number(governance.config?.entry_rate_limit_burst || 0);
    dynamicRulesCount.value =
      Number(governance.dynamic_rules?.red_line_count || 0) +
      Number(governance.dynamic_rules?.stage_requirement_count || 0) +
      Number(governance.dynamic_rules?.quality_compass_count || 0);
    multiUserMode.value = String(governance.multi_user_server?.mode || "single_user");
    multiUserReady.value = governance.multi_user_server?.release_gate?.ready === true;
    multiUserLifecycleReady.value = governance.multi_user_server?.lifecycle?.ready === true;
    dualTrackConsistencyReady.value = governance.dual_track_consistency?.ready === true;
    multiUserSource.value = String(governance.multi_user_server?.inference?.source || "default");

    const puaFailed = Number(governance.violations?.pua_recent_failed || 0);
    const breakerOpen = Number(governance.violations?.breaker_open_count || 0);
    const warningCount = Number(governance.config?.warning_count || 0);
    const strictViolationCount = Number(governance.config?.strict_violation_count || 0);
    const runtimeHealthy = governance.runtime?.is_healthy === true;

    try {
      const providerParsed = await getProviderStatus();
      const providerSummary = providerParsed?.provider_status?.summary || {};
      providerReadyCount.value = Number(providerSummary.ready || 0);
      providerDegradedCount.value = Number(providerSummary.degraded || 0);
      providerConfiguredCount.value = Number(providerSummary.configured || 0);
    } catch {
      providerReadyCount.value = 0;
      providerDegradedCount.value = 0;
      providerConfiguredCount.value = 0;
    }

    try {
      const releaseReadiness = await getReleaseReadiness();
      const readiness = releaseReadiness?.readiness || {};
      releaseReadinessStatus.value = String(readiness.status || "unknown");
      readinessSchemaVersion.value = String(
        readiness.schema_version || readiness.artifact_contract?.schema_version || readiness.version || "unknown",
      );
      releaseBlockedGateCount.value = Number(readiness.blocked_gate_count || 0);
      const blockedNames = Array.isArray(readiness.blocked_gate_names)
        ? readiness.blocked_gate_names.map((item) => String(item))
        : [];
      blockedGateNamesText.value = blockedNames.join("|");
      multiUserMode.value = String(readiness.multi_user_server?.mode || multiUserMode.value);
      multiUserReady.value = readiness.multi_user_server?.release_gate_ready === true;
      multiUserLifecycleReady.value =
        readiness.multi_user_server?.lifecycle?.ready === true ||
        readiness.summary?.multi_user_lifecycle_ready === true;
      dualTrackConsistencyReady.value =
        readiness.dual_track_consistency?.ready === true ||
        readiness.summary?.dual_track_consistency_ready === true;
      multiUserSource.value = String(
        readiness.multi_user_server?.inference?.source ||
          readiness.summary?.multi_user_inference_source ||
          multiUserSource.value,
      );
    } catch {
      releaseReadinessStatus.value = "unknown";
      readinessSchemaVersion.value = "unknown";
      releaseBlockedGateCount.value = 1;
      blockedGateNamesText.value = "";
      multiUserReady.value = false;
      multiUserLifecycleReady.value = false;
      dualTrackConsistencyReady.value = false;
      multiUserSource.value = "default";
    }

    const governancePenalty = Math.min(
      MAX_GOVERNANCE_PENALTY,
      puaFailed * PENALTY_PUA_FAILED +
        breakerOpen * PENALTY_BREAKER_OPEN +
        warningCount * PENALTY_WARNING +
        strictViolationCount * PENALTY_STRICT_VIOLATION +
        providerDegradedCount.value * PENALTY_PROVIDER_DEGRADED +
        releaseBlockedGateCount.value * PENALTY_RELEASE_BLOCKED_GATE +
        (strictEnabled.value ? 0 : PENALTY_STRICT_DISABLED) +
        (entryAuthEnabled.value ? 0 : PENALTY_ENTRY_AUTH_DISABLED) +
        (entryAuthKeyConfigured.value || !entryAuthEnabled.value ? 0 : PENALTY_ENTRY_AUTH_KEY_MISSING),
    );
    overallScore.value = Math.max(MIN_GOVERNANCE_SCORE, runtimeHealthy ? BASE_HEALTHY_SCORE - governancePenalty : BASE_UNHEALTHY_SCORE - governancePenalty);
    credsScore.value = Math.max(MIN_CREDS_SCORE, BASE_CREDS_SCORE - warningCount * PENALTY_CREDS_WARNING - strictViolationCount * PENALTY_CREDS_VIOLATION);
    auditScore.value = Math.max(MIN_AUDIT_SCORE, BASE_AUDIT_SCORE - puaFailed * PENALTY_AUDIT_PUA - breakerOpen * PENALTY_AUDIT_BREAKER);
    configScore.value = Math.max(MIN_CONFIG_SCORE, BASE_CONFIG_SCORE - warningCount * PENALTY_CONFIG_WARNING - strictViolationCount * PENALTY_CONFIG_VIOLATION - (strictEnabled.value ? 0 : PENALTY_CONFIG_STRICT_DISABLED));

    const files = Array.isArray(governance.rules?.files) ? governance.rules.files : [];
    sensitiveFields.value = files.map((item) => ({
      name: String(item.path || "unknown"),
      location: "RULES",
      status: "masked",
      value: `size:${Number(item.size_bytes || 0)} bytes`,
      actual_length: String(item.path || "").length,
    }));

    const nextRisks: Array<{ id: string; type: "warning" | "info" | "success"; title: string; description: string; action: boolean }> = [];
    if (puaFailed > 0) {
      nextRisks.push({
        id: "pua_failed",
        type: "warning",
        title: t("security.riskPuaFailedTitle", { count: puaFailed }),
        description: t("security.riskPuaFailedDesc"),
        action: true,
      });
    }
    if (breakerOpen > 0) {
      nextRisks.push({
        id: "breaker_open",
        type: "warning",
        title: t("security.riskBreakerOpenTitle", { count: breakerOpen }),
        description: t("security.riskBreakerOpenDesc"),
        action: true,
      });
    }
    if (warningCount > 0) {
      nextRisks.push({
        id: "config_warn",
        type: "info",
        title: t("security.riskConfigWarnTitle", { count: warningCount }),
        description: t("security.riskConfigWarnDesc"),
        action: false,
      });
    }
    if (!strictEnabled.value) {
      nextRisks.push({
        id: "strict_disabled",
        type: "warning",
        title: t("security.riskStrictDisabledTitle"),
        description: t("security.riskStrictDisabledDesc"),
        action: true,
      });
    }
    if (entryAuthEnabled.value && !entryAuthKeyConfigured.value) {
      nextRisks.push({
        id: "entry_auth_key_missing",
        type: "warning",
        title: t("security.riskEntryAuthKeyMissingTitle"),
        description: t("security.riskEntryAuthKeyMissingDesc"),
        action: true,
      });
    }
    if (!entryAuthEnabled.value) {
      nextRisks.push({
        id: "entry_auth_disabled",
        type: "info",
        title: t("security.riskEntryAuthDisabledTitle"),
        description: t("security.riskEntryAuthDisabledDesc"),
        action: true,
      });
    }
    if (providerDegradedCount.value > 0) {
      nextRisks.push({
        id: "provider_degraded",
        type: "warning",
        title: t("security.riskProviderDegradedTitle", { count: providerDegradedCount.value }),
        description: t("security.riskProviderDegradedDesc"),
        action: true,
      });
    }
    if (releaseBlockedGateCount.value > 0) {
      nextRisks.push({
        id: "release_blocked",
        type: "warning",
        title: t("security.riskReleaseBlockedTitle", { count: releaseBlockedGateCount.value }),
        description: t("security.riskReleaseBlockedDesc"),
        action: true,
      });
    }
    if (strictViolationCount > 0) {
      nextRisks.push({
        id: "strict_violation",
        type: "warning",
        title: t("security.riskStrictViolationTitle", { count: strictViolationCount }),
        description: t("security.riskStrictViolationDesc"),
        action: true,
      });
    }
    if (nextRisks.length === 0) {
      nextRisks.push({
        id: "healthy",
        type: "success",
        title: t("security.riskHealthyTitle"),
        description: t("security.riskHealthyDesc"),
        action: false,
      });
    }
    risks.splice(0, risks.length, ...nextRisks);

    await refreshGovernanceAuditRecent(20);
    if (auditLogs.value.length === 0) {
      auditLogs.value = [
        {
          timestamp: new Date().toLocaleString(),
          action: "governance.status",
          resource: rulesVersion.value,
          user: "system",
          result: runtimeHealthy ? "success" : "danger",
        },
      ];
    }
  } catch (error) {
    ElMessage.error(t("security.governanceStatusFailed", { error: normalizeErrorMessage(error) }));
  }
}

async function auditSensitiveFields() {
  ElMessage.info(t("security.auditRunning"));
  await refreshGovernanceStatus();
  await refreshGovernanceAuditRecent(50);
  ElMessage.success(t("security.auditComplete"));
}

async function fixRisk(riskId: string) {
  ElMessageBox.confirm(
    t("security.confirmFix") + " " + t("security.fixLocalOnlyWarning"),
    t("security.fixWarning"),
    {
      confirmButtonText: t("common.confirm"),
      cancelButtonText: t("common.cancel"),
      type: "warning",
    },
  )
    .then(async () => {
      try {
        await remediateRisk(riskId);
        // Only remove locally if backend confirms the fix
        const idx = risks.findIndex((r) => r.id === riskId);
        if (idx >= 0) {
          risks.splice(idx, 1);
        }
        ElMessage.success(t("security.fixSuccess"));
      } catch (err) {
        ElMessage.error(t("security.fixRiskFailed") || `Failed to fix risk: ${normalizeErrorMessage(err)}`);
      }
    })
    .catch(() => {
      ElMessage.info(t("common.cancelled"));
    });
}

async function exportAuditLog() {
  try {
    const csv = convertToCSV(auditLogs.value);
    const blob = new Blob([csv], { type: "text/csv" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `audit_log_${new Date().getTime()}.csv`;
    link.click();
    URL.revokeObjectURL(url);
    ElMessage.success(t("security.exportSuccess"));
  } catch (err) {
    ElMessage.error(normalizeErrorMessage(err));
  }
}

function convertToCSV(data: Array<Record<string, unknown>>): string {
  if (data.length === 0) return "";
  const headers = Object.keys(data[0]);
  const rows = data.map((item) => headers.map((header) => `"${String(item[header] ?? "")}"`).join(","));
  return [headers.join(","), ...rows].join("\n");
}

async function saveSecurity() {
  try {
    await saveGovernanceConfig({
      autoMaskSensitive: autoMaskSensitive.value,
      auditEnabled: auditEnabled.value,
    });
    ElMessage.success(t("security.settingsSaved"));
  } catch {
    // Backend save failed — persist locally as fallback
    try {
      localStorage.setItem(
        "goon.gui.securitySettings",
        JSON.stringify({
          autoMaskSensitive: autoMaskSensitive.value,
          auditEnabled: auditEnabled.value,
        }),
      );
    } catch {}
    ElMessage.warning(t("security.settingsSavedLocally") || "Settings saved locally only");
  }
}

onMounted(async () => {
  await refreshGovernanceStatus();
  ElMessage.info(
    t("security.governanceToast", {
      state: governanceState.value,
      level: normalizeLevelTag(governanceState.value),
      rules: rulesVersion.value,
    })
  );
});
</script>
