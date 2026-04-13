<template>
  <el-card>
    <template #header>{{ t("setup.title") }}</template>
    <el-space direction="vertical" alignment="start">
      <el-text>{{ t("setup.hint") }}</el-text>
      <el-space>
        <el-button type="primary" @click="run('--init --setup-level quick')">{{ t("setup.quick") }}</el-button>
        <el-button @click="run('--init --setup-level standard')">{{ t("setup.standard") }}</el-button>
        <el-button @click="run('--init --setup-level custom')">{{ t("setup.custom") }}</el-button>
      </el-space>
      <el-space>
        <el-button @click="run('--check')">{{ t("setup.check") }}</el-button>
        <el-button @click="run('--doctor')">{{ t("setup.doctor") }}</el-button>
        <el-button @click="run('--apply-recommended')">{{ t("setup.applyRecommended") }}</el-button>
        <el-button @click="run('--status')">{{ t("setup.status") }}</el-button>
        <el-button @click="run('--healthcheck')">{{ t("setup.healthcheck") }}</el-button>
      </el-space>
      <el-space>
        <el-button type="warning" @click="restoreDefaults">{{ t("setup.restoreDefaults") }}</el-button>
      </el-space>
      <el-space>
        <el-input v-model="customArgs" size="small" style="width: 360px" :placeholder="t('setup.customArgs')" />
        <el-button @click="run(customArgs)">{{ t("setup.runCustom") }}</el-button>
      </el-space>
      <el-input v-model="output" type="textarea" :rows="12" readonly />
    </el-space>
  </el-card>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { resetDefaultSettings, runCliCommand } from "../services/bridge";

const output = ref("");
const customArgs = ref("");
const { t } = useI18n();

async function run(args: string) {
  if (!args || !args.trim()) {
    return;
  }
  try {
    output.value = await runCliCommand(args);
    ElMessage.success(t("setup.commandCompleted"));
  } catch (error) {
    output.value = String(error);
    ElMessage.error(t("setup.commandFailed"));
  }
}

async function restoreDefaults() {
  try {
    output.value = await resetDefaultSettings();
    ElMessage.success(t("setup.defaultsRestored"));
  } catch (error) {
    output.value = String(error);
    ElMessage.error(t("setup.commandFailed"));
  }
}
</script>
