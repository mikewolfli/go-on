<template>
  <el-card>
    <template #header>
      <div style="display:flex;justify-content:space-between;align-items:center;">
        <span>{{ t("logs.title") }}</span>
        <el-space>
          <el-tag v-if="runtime.logsStale" type="warning">{{ t("common.staleData") }}</el-tag>
          <el-input v-model="keyword" size="small" :placeholder="t('logs.filter')" style="width: 200px" />
          <el-button size="small" @click="runtime.refreshLogs(300)">{{ t("app.refresh") }}</el-button>
        </el-space>
      </div>
    </template>
    <div style="max-height: 65vh; overflow:auto; font-family: Consolas, monospace; font-size: 12px;">
      <div v-for="(line, idx) in filteredLines" :key="idx">{{ line }}</div>
    </div>
  </el-card>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";

const runtime = useRuntimeStore();
const { t } = useI18n();
const keyword = ref("");

const filteredLines = computed(() => {
  const query = keyword.value.trim().toLowerCase();
  if (!query) return runtime.logs.lines;
  return runtime.logs.lines.filter((line) => line.toLowerCase().includes(query));
});

onMounted(() => runtime.startLogsPolling(300));
onUnmounted(() => runtime.stopLogsPolling());
</script>
