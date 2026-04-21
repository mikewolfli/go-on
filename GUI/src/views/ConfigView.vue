<template>
  <el-card>
    <template #header>{{ t("config.title") }}</template>
    <el-form label-width="150px">
      <el-form-item :label="t('config.executable')">
        <el-input v-model="executablePath" placeholder="D:/Workspace/RustWorkspace/go-on/go-on.exe">
          <template #append>
            <el-button @click="pickExecutable">选择文件</el-button>
          </template>
        </el-input>
      </el-form-item>
      <el-form-item :label="t('config.workingDir')">
        <el-input v-model="workingDir" placeholder="D:/Workspace/RustWorkspace/go-on">
          <template #append>
            <el-button @click="pickWorkingDir">选择目录</el-button>
          </template>
        </el-input>
      </el-form-item>
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
          <div style="font-weight: 600; margin-bottom: 8px;">{{ selectedMode?.label }}</div>
          <div style="margin-bottom: 6px;">{{ selectedMode?.description }}</div>
          <el-text size="small" type="success">{{ selectedMode?.recommendedFor }}</el-text>
        </el-card>
      </el-form-item>
      <el-form-item :label="t('config.monitorOnly')">
        <el-switch v-model="monitorOnly" />
        <el-text style="margin-left: 10px;" size="small">{{ t('config.monitorOnlyHint') }}</el-text>
      </el-form-item>
      <el-form-item>
        <el-button type="primary" @click="save">{{ t("config.save") }}</el-button>
      </el-form-item>
    </el-form>
  </el-card>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { configureService, serviceStatus } from "../services/bridge";
import { openDialog } from "../services/dialog";
import { normalizeErrorMessage } from "../utils/errors";

const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";
const PROTOCOL_MODE_KEY = "goon.gui.protocolMode";
const executablePath = ref("go-on");
const workingDir = ref(".");
const protocolMode = ref(localStorage.getItem(PROTOCOL_MODE_KEY) || "from_config");
const monitorOnly = ref(localStorage.getItem(MONITOR_ONLY_KEY) === "true");
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
});

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
    await configureService(executablePath.value, workingDir.value, protocolMode.value);
    localStorage.setItem(MONITOR_ONLY_KEY, String(monitorOnly.value));
    localStorage.setItem(PROTOCOL_MODE_KEY, protocolMode.value);
    window.dispatchEvent(new CustomEvent<boolean>("goon:monitor-only-changed", { detail: monitorOnly.value }));
    ElMessage.success(t("config.saved"));
  } catch (error) {
    ElMessage.error(normalizeErrorMessage(error));
  }
}
</script>
