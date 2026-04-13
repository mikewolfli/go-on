<template>
  <el-card>
    <template #header>{{ t("config.title") }}</template>
    <el-form label-width="150px">
      <el-form-item :label="t('config.executable')">
        <el-input v-model="executablePath" placeholder="D:/Workspace/RustWorkspace/go-on/go-on.exe" />
      </el-form-item>
      <el-form-item :label="t('config.workingDir')">
        <el-input v-model="workingDir" placeholder="D:/Workspace/RustWorkspace/go-on" />
      </el-form-item>
      <el-form-item>
        <el-button type="primary" @click="save">{{ t("config.save") }}</el-button>
      </el-form-item>
    </el-form>
  </el-card>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { configureService } from "../services/bridge";

const executablePath = ref("go-on");
const workingDir = ref(".");
const { t } = useI18n();

async function save() {
  try {
    await configureService(executablePath.value, workingDir.value);
    ElMessage.success(t("config.saved"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}
</script>
