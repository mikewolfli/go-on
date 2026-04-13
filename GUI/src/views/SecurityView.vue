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
            <el-row :gutter="16">
              <el-col :span="6">
                <el-statistic :title="t('security.overallScore')" :value="overallScore" suffix="/100" />
              </el-col>
              <el-col :span="6">
                <el-statistic :title="t('security.credsScore')" :value="credsScore" suffix="/100" />
              </el-col>
              <el-col :span="6">
                <el-statistic :title="t('security.auditScore')" :value="auditScore" suffix="/100" />
              </el-col>
              <el-col :span="6">
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
import { ref, reactive } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { useI18n } from "vue-i18n";

const { t } = useI18n();

const overallScore = ref(78);
const credsScore = ref(85);
const auditScore = ref(72);
const configScore = ref(76);

const autoMaskSensitive = ref(true);
const auditEnabled = ref(true);

const sensitiveFields = ref([
  { name: "OPENAI_API_KEY", location: ".env.goon", status: "masked", value: "sk-...***...xyz", actual_length: 48 },
  { name: "ANTHROPIC_API_KEY", location: ".env.goon", status: "masked", value: "sk-ant-...***...abc", actual_length: 51 },
  { name: "GITHUB_TOKEN", location: "env:GITHUB_TOKEN", status: "masked", value: "ghp_...***...def", actual_length: 36 },
  { name: "database.password", location: "config.toml", status: "masked", value: "***", actual_length: 12 },
]);

const risks = reactive([
  {
    id: "risk_001",
    type: "warning",
    title: t("security.risk1Title"),
    description: t("security.risk1Desc"),
    action: true,
  },
  {
    id: "risk_002",
    type: "info",
    title: t("security.risk2Title"),
    description: t("security.risk2Desc"),
    action: false,
  },
  {
    id: "risk_003",
    type: "success",
    title: t("security.risk3Title"),
    description: t("security.risk3Desc"),
    action: false,
  },
]);

const auditLogs = ref([
  {
    timestamp: "2026-04-13 14:45:30",
    action: "api_key_set",
    resource: "OPENAI_API_KEY",
    user: "admin",
    result: "success",
  },
  {
    timestamp: "2026-04-13 14:30:15",
    action: "config_read",
    resource: "config.toml",
    user: "anonymous",
    result: "success",
  },
  {
    timestamp: "2026-04-13 14:15:00",
    action: "api_key_clear",
    resource: "ANTHROPIC_API_KEY",
    user: "admin",
    result: "success",
  },
  {
    timestamp: "2026-04-13 14:00:45",
    action: "audit_log_export",
    resource: "audit.log",
    user: "admin",
    result: "success",
  },
]);

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

async function auditSensitiveFields() {
  ElMessage.info(t("security.auditRunning"));
  // Simulating audit
  setTimeout(() => {
    ElMessage.success(t("security.auditComplete"));
  }, 2000);
}

async function fixRisk(riskId: string) {
  ElMessageBox.confirm(t("security.confirmFix"), t("security.fixWarning"), {
    confirmButtonText: t("common.confirm"),
    cancelButtonText: t("common.cancel"),
    type: "warning",
  })
    .then(() => {
      ElMessage.success(t("security.fixSuccess"));
      // Remove risk from list
      const idx = risks.findIndex((r) => r.id === riskId);
      if (idx >= 0) {
        risks.splice(idx, 1);
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
    ElMessage.error(`Error: ${err}`);
  }
}

function convertToCSV(data: any[]): string {
  if (data.length === 0) return "";
  const headers = Object.keys(data[0]);
  const rows = data.map((item) => headers.map((header) => `"${item[header]}"`).join(","));
  return [headers.join(","), ...rows].join("\n");
}

async function saveSecurity() {
  ElMessage.success(t("security.settingsSaved"));
}
</script>
