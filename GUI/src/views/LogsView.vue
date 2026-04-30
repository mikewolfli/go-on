<template>
  <el-card>
    <template #header>
      <div style="display:flex;justify-content:space-between;align-items:center;">
        <span>{{ t("logs.title") }}</span>
        <el-space>
          <el-tag v-if="runtime.logsStale" type="warning">{{ t("common.staleData") }}</el-tag>
          <el-select v-model="selectedLevel" size="small" style="width:110px" clearable :placeholder="t('logs.level')">
                <el-option v-for="lvl in logLevels" :key="lvl" :label="lvl" :value="lvl" />
          </el-select>
          <el-input v-model="keyword" size="small" :placeholder="t('logs.filter')" style="width: 200px" />
          <el-button size="small" @click="toggleAutoScroll" :type="autoScroll ? 'primary' : 'default'">
            {{ autoScroll ? t('views.LogsView.autoScroll') + ' ON' : t('views.LogsView.autoScroll') + ' OFF' }}
          </el-button>
          <el-button size="small" :disabled="filteredLines.length === 0" @click="exportLogs">
            {{ t('logs.export') }}
          </el-button>
          <el-badge v-if="hasServerSearch" :value="t('common.name')" type="info" style="vertical-align:middle">
            <el-tag size="small" type="info" effect="plain">{{ t('logs.serverSearch') }}</el-tag>
          </el-badge>
          <el-button size="small" @click="runtime.refreshLogs(300)">{{ t("app.refresh") }}</el-button>
        </el-space>
      </div>
    </template>
    <div ref="logContainer" style="max-height: 65vh; overflow:auto; font-family: Consolas, monospace; font-size: 12px;">
      <div v-if="!runtime.logs.lines.length && !runtime.loading" style="text-align:center;padding:20px;color:#999;">
        {{ t('views.LogsView.noLogs') || 'No log entries yet' }}
      </div>
      <div v-for="(line, idx) in filteredLines" :key="idx">
        <span style="color:#999">{{ formatTimestamp(idx) }}</span>
        <span :class="logLevelClass(line)">{{ line }}</span>
      </div>
    </div>
  </el-card>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, nextTick, watch } from "vue";
import { ElMessage } from "element-plus";
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";

const runtime = useRuntimeStore();
const { t } = useI18n();
const keyword = ref("");
const selectedLevel = ref("");
const autoScroll = ref(true);
const logContainer = ref<HTMLElement | null>(null);

const logLevels = ["DEBUG", "INFO", "WARN", "ERROR", "TRACE"];

// Detect if logs appear to have level prefixes (server-side structured logs)
const hasServerSearch = computed(() => {
  return runtime.logs.lines.some((line) =>
    logLevels.some((lvl) => line.includes(`[${lvl}]`) || line.includes(` ${lvl} `)),
  );
});

// Simple heuristic: extract level from line if present
function getLogLevel(line: string): string | null {
  const upper = line.toUpperCase();
  for (const lvl of logLevels) {
    if (upper.includes(`[${lvl}]`) || upper.includes(` ${lvl} `) || upper.startsWith(lvl)) {
      return lvl;
    }
  }
  return null;
}

function logLevelClass(line: string): string {
  const lvl = getLogLevel(line);
  if (!lvl) return "";
  const cls = `log-${lvl.toLowerCase()}`;
  return cls;
}

const filteredLines = computed(() => {
  let lines = runtime.logs.lines;
  const query = keyword.value.trim().toLowerCase();
  if (query) {
    lines = lines.filter((line) => line.toLowerCase().includes(query));
  }
  const lvl = selectedLevel.value;
  if (lvl) {
    lines = lines.filter((line) => getLogLevel(line) === lvl);
  }
  return lines;
});

// Auto-scroll: watch filteredLines and scroll to bottom
const stopWatchingLogs = watch(filteredLines, () => {
  if (autoScroll.value) {
    void nextTick(() => {
      if (logContainer.value) {
        logContainer.value.scrollTop = logContainer.value.scrollHeight;
      }
    });
  }
});

function toggleAutoScroll() {
  autoScroll.value = !autoScroll.value;
}

function formatTimestamp(_idx: number): string {
  // Use current time as a display prefix hint; actual timestamps
  // would come from structured logs. For now show a relative index marker.
  return `[${new Date().toLocaleTimeString()}] `;
}

function exportLogs() {
  const content = filteredLines.value.join("\n");
  const blob = new Blob([content], { type: "text/plain" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `go-on-logs-${new Date().toISOString().slice(0, 19)}.txt`;
  a.click();
  URL.revokeObjectURL(url);
}

onMounted(() => runtime.startLogsPolling(300));
onUnmounted(() => {
  runtime.stopLogsPolling();
  stopWatchingLogs();
});
</script>

<style scoped>
.log-debug { color: #888; }
.log-info { color: #0af; }
.log-warn { color: #fa0; }
.log-error { color: #f44; font-weight: bold; }
.log-trace { color: #aaa; font-style: italic; }
</style>
