<template>
  <el-card>
    <template #header>{{ t("backendOps.title") }}</template>
    <el-space direction="vertical" alignment="start" style="width:100%">
      <el-text>{{ t("backendOps.hint") }}</el-text>

      <el-space wrap>
        <el-button @click="call('breaker.status')">breaker.status</el-button>
        <el-button type="warning" @click="call('breaker.reset')">breaker.reset</el-button>
        <el-button @click="call('config.reload')">config.reload</el-button>
        <el-button type="danger" @click="call('shutdown')">shutdown</el-button>
      </el-space>

      <el-space wrap>
        <el-button @click="call('cache.clear')">cache.clear</el-button>
        <el-button @click="call('vector.clear')">vector.clear</el-button>
      </el-space>

      <el-space wrap>
        <el-button @click="call('metrics.get')">metrics.get</el-button>
        <el-button @click="call('metrics.prometheus')">metrics.prometheus</el-button>
      </el-space>

      <el-space wrap>
        <el-button @click="call('trace.get')">trace.get</el-button>
        <el-button @click="call('trace.metrics')">trace.metrics</el-button>
        <el-button @click="call('debug_panel.get')">debug_panel.get</el-button>
      </el-space>

      <el-divider />

      <el-space>
        <el-input v-model="customMethod" style="width: 240px" :placeholder="t('backendOps.customMethod')" />
        <el-input v-model="customParams" style="width: 360px" :placeholder="t('backendOps.customParams')" />
        <el-button type="primary" @click="callCustom">{{ t("backendOps.run") }}</el-button>
      </el-space>

      <el-input v-model="output" type="textarea" :rows="16" readonly />
    </el-space>
  </el-card>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { invokeRuntimeRpc } from "../services/bridge";

const { t } = useI18n();
const output = ref("");
const customMethod = ref("");
const customParams = ref("{}");

async function call(method: string, params = "{}") {
  try {
    output.value = await invokeRuntimeRpc(method, params);
    ElMessage.success(t("backendOps.done"));
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}

async function callCustom() {
  if (!customMethod.value.trim()) {
    return;
  }
  await call(customMethod.value.trim(), customParams.value || "{}");
}
</script>
