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
import { onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { configureService, serviceStatus } from "../services/bridge";
import { openDialog } from "../services/dialog";

const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";
const executablePath = ref("go-on");
const workingDir = ref(".");
const monitorOnly = ref(localStorage.getItem(MONITOR_ONLY_KEY) === "true");
const { t } = useI18n();

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
    await configureService(executablePath.value, workingDir.value);
    localStorage.setItem(MONITOR_ONLY_KEY, String(monitorOnly.value));
    window.dispatchEvent(new CustomEvent<boolean>("goon:monitor-only-changed", { detail: monitorOnly.value }));
    ElMessage.success(t("config.saved"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}
</script>
