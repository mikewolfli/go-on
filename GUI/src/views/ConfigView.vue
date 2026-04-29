<template>
  <el-card>
    <template #header>
      <div class="config-header">
        <span>{{ t("config.title") }}</span>
        <el-button size="small" @click="wizardVisible = true">{{ t("config.wizard.open") }}</el-button>
      </div>
    </template>

    <ConfigWizard
      v-model="wizardVisible"
      :executable-path="executablePath"
      :working-dir="workingDir"
      :protocol-mode="protocolMode"
      :monitor-only="monitorOnly"
      @apply="applyWizard"
    />

    <el-alert :title="t('config.summaryHint')" type="info" :closable="false" show-icon class="config-alert" />

    <el-collapse v-model="activePanels">
      <el-collapse-item :title="t('config.runtimeGroup')" name="runtime">
        <el-form label-width="150px">
          <el-form-item :label="t('config.executable')">
            <el-input v-model="executablePath" :placeholder="t('config.executablePathPlaceholder')">
              <template #append>
                <el-button @click="pickExecutable">{{ t('config.pickFile') }}</el-button>
              </template>
            </el-input>
          </el-form-item>
          <el-form-item :label="t('config.workingDir')">
            <el-input v-model="workingDir" placeholder="D:/Workspace/RustWorkspace/go-on">
              <template #append>
                <el-button @click="pickWorkingDir">{{ t('config.pickDirectory') }}</el-button>
              </template>
            </el-input>
          </el-form-item>
        </el-form>
      </el-collapse-item>

      <el-collapse-item :title="t('config.protocolGroup')" name="protocol">
        <el-form label-width="150px">
          <el-form-item :label="t('config.protocolMode')">
            <el-select v-model="protocolMode" style="width: 100%;">
              <el-option
                v-for="mode in protocolModes"
                :key="mode.value"
                :label="mode.label"
                :value="mode.value"
              />
            </el-select>
          </el-form-item>
          <el-form-item>
            <el-card shadow="never" style="width: 100%;">
              <div class="mode-title">{{ selectedMode?.label }}</div>
              <div class="mode-description">{{ selectedMode?.description }}</div>
              <el-text size="small" type="success">{{ selectedMode?.recommendedFor }}</el-text>
            </el-card>
          </el-form-item>
        </el-form>
      </el-collapse-item>

      <el-collapse-item :title="t('config.behaviorGroup')" name="behavior">
        <el-form label-width="150px">
          <el-form-item :label="t('config.monitorOnly')">
            <el-switch v-model="monitorOnly" />
            <el-text style="margin-left: 10px;" size="small">{{ t('config.monitorOnlyHint') }}</el-text>
          </el-form-item>
          <el-form-item :label="t('config.diagnose')">
            <el-button :loading="diagnosing" @click="runDiagnose">{{ t('config.runDiagnose') }}</el-button>
          </el-form-item>
          <el-form-item v-if="diagnoseOutput" :label="t('config.diagnoseResult')">
            <el-input v-model="diagnoseOutput" type="textarea" :rows="8" readonly />
          </el-form-item>
        </el-form>
      </el-collapse-item>
    </el-collapse>

    <div class="save-row">
      <el-button type="primary" @click="save">{{ t("config.save") }}</el-button>
    </div>
  </el-card>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { useI18n } from "vue-i18n";
import { configureService, runCliCommand, serviceStatus } from "../services/bridge";
import { openDialog } from "../services/dialog";
import { normalizeErrorMessage } from "../utils/errors";
import ConfigWizard, { type ConfigWizardDraft } from "../components/ConfigWizard.vue";

const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";
const PROTOCOL_MODE_KEY = "goon.gui.protocolMode";
const executablePath = ref("go-on");
const workingDir = ref(".");
const protocolMode = ref(localStorage.getItem(PROTOCOL_MODE_KEY) || "from_config");
const monitorOnly = ref(localStorage.getItem(MONITOR_ONLY_KEY) === "true");
const activePanels = ref(["runtime", "protocol"]);
const wizardVisible = ref(false);
const diagnosing = ref(false);
const diagnoseOutput = ref("");
const initialState = ref({
  executablePath: "go-on",
  workingDir: ".",
  protocolMode: localStorage.getItem(PROTOCOL_MODE_KEY) || "from_config",
  monitorOnly: localStorage.getItem(MONITOR_ONLY_KEY) === "true",
});
const { t } = useI18n();

const protocolModes = computed(() => [
  {
    value: "from_config",
    label: t("config.protocolModeFromConfig"),
    description: t("config.protocolModeFromConfigDesc"),
    recommendedFor: t("config.protocolModeFromConfigRecommended"),
  },
  {
    value: "adaptive",
    label: "adaptive",
    description: t("protocol_mode.adaptive.description"),
    recommendedFor: t("protocol_mode.adaptive.recommended_for"),
  },
  {
    value: "acp_stdio",
    label: "acp_stdio",
    description: t("protocol_mode.acp_stdio.description"),
    recommendedFor: t("protocol_mode.acp_stdio.recommended_for"),
  },
  {
    value: "acp_http",
    label: "acp_http",
    description: t("protocol_mode.acp_http.description"),
    recommendedFor: t("protocol_mode.acp_http.recommended_for"),
  },
  {
    value: "mcp_stdio",
    label: "mcp_stdio",
    description: t("protocol_mode.mcp_stdio.description"),
    recommendedFor: t("protocol_mode.mcp_stdio.recommended_for"),
  },
  {
    value: "mcp_http",
    label: "mcp_http",
    description: t("protocol_mode.mcp_http.description"),
    recommendedFor: t("protocol_mode.mcp_http.recommended_for"),
  },
]);

const selectedMode = computed(() => protocolModes.value.find((item) => item.value === protocolMode.value));

onMounted(async () => {
  try {
    const status = await serviceStatus();
    if (status.executablePath) {
      executablePath.value = status.executablePath;
    }
    if (status.workingDir) {
      workingDir.value = status.workingDir;
    }
  } catch {
    // Keep defaults if status cannot be loaded.
  }
  initialState.value = {
    executablePath: executablePath.value,
    workingDir: workingDir.value,
    protocolMode: protocolMode.value,
    monitorOnly: monitorOnly.value,
  };
});

function applyWizard(draft: ConfigWizardDraft) {
  executablePath.value = draft.executablePath;
  workingDir.value = draft.workingDir;
  protocolMode.value = draft.protocolMode;
  monitorOnly.value = draft.monitorOnly;
}

async function pickExecutable() {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    title: "选择 go-on 后台可执行文件",
    filters: [
      { name: "Executable", extensions: ["exe", "bin", ""] },
      { name: "All Files", extensions: ["*"] },
    ],
  });

  if (!picked) {
    return;
  }
  const selected = Array.isArray(picked) ? picked[0] : picked;
  if (selected && String(selected).trim()) {
    executablePath.value = String(selected);
  }
}

async function pickWorkingDir() {
  const picked = await openDialog({
    multiple: false,
    directory: true,
    title: "选择工作目录",
  });

  if (!picked) {
    return;
  }
  const selected = Array.isArray(picked) ? picked[0] : picked;
  if (selected && String(selected).trim()) {
    workingDir.value = String(selected);
  }
}

async function save() {
  try {
    const changes = [
      initialState.value.executablePath !== executablePath.value ? `${t("config.executable")}: ${initialState.value.executablePath} -> ${executablePath.value}` : null,
      initialState.value.workingDir !== workingDir.value ? `${t("config.workingDir")}: ${initialState.value.workingDir} -> ${workingDir.value}` : null,
      initialState.value.protocolMode !== protocolMode.value ? `${t("config.protocolMode")}: ${initialState.value.protocolMode} -> ${protocolMode.value}` : null,
      initialState.value.monitorOnly !== monitorOnly.value ? `${t("config.monitorOnly")}: ${initialState.value.monitorOnly ? t("config.yes") : t("config.no")} -> ${monitorOnly.value ? t("config.yes") : t("config.no")}` : null,
    ].filter((item): item is string => Boolean(item));

    if (changes.length > 0) {
      await ElMessageBox.confirm(changes.join("\n"), t("config.changeSummaryTitle"), {
        confirmButtonText: t("config.confirmSave"),
        cancelButtonText: t("config.cancelSave"),
        type: "warning",
      });
    }
    await configureService(executablePath.value, workingDir.value, protocolMode.value);
    localStorage.setItem(MONITOR_ONLY_KEY, String(monitorOnly.value));
    localStorage.setItem(PROTOCOL_MODE_KEY, protocolMode.value);
    window.dispatchEvent(new CustomEvent<boolean>("goon:monitor-only-changed", { detail: monitorOnly.value }));
    initialState.value = {
      executablePath: executablePath.value,
      workingDir: workingDir.value,
      protocolMode: protocolMode.value,
      monitorOnly: monitorOnly.value,
    };
    ElMessage.success(t("config.saved"));
  } catch (error) {
    if (error === "cancel") {
      return;
    }
    ElMessage.error(normalizeErrorMessage(error));
  }
}

async function runDiagnose() {
  diagnosing.value = true;
  try {
    const output = await runCliCommand("--diagnose");
    diagnoseOutput.value = output;
    ElMessage.success(t("config.diagnoseCompleted"));
  } catch (error) {
    const normalized = normalizeErrorMessage(error);
    diagnoseOutput.value = normalized;
    ElMessage.error(normalized);
  } finally {
    diagnosing.value = false;
  }
}
</script>

<style scoped>
.config-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.config-alert {
  margin-bottom: 14px;
}

.mode-title {
  font-weight: 600;
  margin-bottom: 8px;
}

.mode-description {
  margin-bottom: 6px;
}

.save-row {
  margin-top: 16px;
  display: flex;
  justify-content: flex-end;
}
</style>
