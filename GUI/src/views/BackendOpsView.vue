<template>
  <el-card>
    <template #header>{{ t("backendOps.title") }}</template>
    <el-space direction="vertical" alignment="start" style="width:100%">
      <el-text>{{ t("backendOps.hint") }}</el-text>

      <el-space wrap>
        <el-button @click="call('breaker.status')">breaker.status</el-button>
        <el-button type="warning" @click="callDangerous('breaker.reset')">breaker.reset</el-button>
        <el-button type="warning" plain @click="call('breaker.recovery', { dry_run: true })">breaker.recovery</el-button>
        <el-button @click="call('config.reload')">config.reload</el-button>
        <el-button @click="call('config.baseline')">{{ t("backendOps.configBaseline") }}</el-button>
        <el-button @click="call('build.repro')">{{ t("backendOps.buildRepro") }}</el-button>
        <el-button @click="call('data.lifecycle')">{{ t("backendOps.dataLifecycle") }}</el-button>
        <el-button @click="call('error.contract')">{{ t("backendOps.errorContract") }}</el-button>
        <el-button @click="call('optimization.peak', { task: 'BLUE15 one-shot optimization peak' })">{{ t("backendOps.optimizationPeak") }}</el-button>
        <el-button type="danger" @click="callDangerous('shutdown')">shutdown</el-button>
      </el-space>

      <el-space wrap>
        <el-button @click="callDangerous('cache.clear')">cache.clear</el-button>
        <el-button @click="callDangerous('vector.clear')">vector.clear</el-button>
      </el-space>

      <el-space wrap>
        <el-button type="success" @click="callQualityBaseline">{{ t("backendOps.qualityBaseline") }}</el-button>
        <el-button type="success" plain @click="call('harness.status', { seed: 20260415 })">harness.status</el-button>
        <el-button type="success" plain @click="call('learning.guardrail', { limit: 50 })">learning.guardrail</el-button>
        <el-button type="success" plain @click="call('knowledge.distill', { limit: 20, strategy_limit: 8 })">{{ t("backendOps.knowledgeDistill") }}</el-button>
        <el-button type="success" plain @click="call('rl.alignment.offline_eval', { window: 120 })">{{ t("backendOps.rlAlignmentEval") }}</el-button>
        <el-button type="success" plain @click="call('hardness.status', { task: 'Assess request routing difficulty', changed_files: ['src/acp/impl/request.rs'], tool_dependencies: ['search_files', 'read_file'] })">{{ t("backendOps.hardnessStatus") }}</el-button>
        <el-button type="success" plain @click="call('cost.status', { task: 'Optimize token budget and model cost routing', changed_files: ['src/acp/impl/request.rs', 'vscode-addon/src/extension.ts'], tool_dependencies: ['search_files', 'read_file', 'write_file'], max_output_tokens: 1800 })">{{ t("backendOps.costStatus") }}</el-button>
        <el-button type="primary" @click="call('selector.status')">{{ t("backendOps.selectorStatus") }}</el-button>
        <el-button type="primary" plain @click="call('learning.replay', { limit: 20 })">{{ t("backendOps.learningReplay") }}</el-button>
        <el-button @click="call('metrics.get')">metrics.get</el-button>
        <el-button @click="call('metrics.prometheus')">metrics.prometheus</el-button>
      </el-space>

      <el-space wrap>
        <el-button @click="call('trace.get')">trace.get</el-button>
        <el-button @click="call('trace.metrics')">trace.metrics</el-button>
        <el-button type="danger" plain @click="call('security.baseline')">security.baseline</el-button>
        <el-button type="warning" plain @click="call('runtime.stability')">{{ t("backendOps.runtimeStability") }}</el-button>
        <el-button type="warning" plain @click="call('observability.alerts', { limit: 20 })">observability.alerts</el-button>
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
import { ElMessage, ElMessageBox } from "element-plus";
import { useI18n } from "vue-i18n";
import { invokeRuntimeRpc } from "../services/bridge";
import { getMetrics } from "../services/rpcService";

const { t } = useI18n();
const output = ref("");
const customMethod = ref("");
const customParams = ref("{}");

function parseRpcOutput(raw: string): any {
  try {
    return JSON.parse(raw);
  } catch {
    return {};
  }
}

async function call(method: string, params: unknown = {}) {
  const payload =
    typeof params === "string" ? (params.trim() ? params : "{}") : JSON.stringify(params ?? {});
  try {
    output.value = await invokeRuntimeRpc(method, payload);
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

async function callDangerous(method: "shutdown" | "cache.clear" | "vector.clear" | "breaker.reset") {
  const consequences: Record<string, string> = {
    "shutdown": "This will stop the backend process and interrupt current requests.",
    "cache.clear": "This will clear runtime cache data.",
    "vector.clear": "This will clear vector storage data.",
    "breaker.reset": "This will force-reset breaker states."
  };

  const confirmed = await ElMessageBox.confirm(
    `${method}\n\n${consequences[method] || "This operation is irreversible."}`,
    "Confirm dangerous operation",
    {
      confirmButtonText: t("common.confirm"),
      cancelButtonText: t("common.cancel"),
      type: "warning",
      closeOnClickModal: false,
      closeOnPressEscape: false,
    }
  ).then(() => true).catch(() => false);

  if (!confirmed) {
    ElMessage.info(t("common.cancelled"));
    return;
  }

  if (method === "shutdown") {
    const typed = await ElMessageBox.prompt(
      'Type "shutdown" to confirm backend shutdown.',
      "Final confirmation",
      {
        confirmButtonText: t("common.confirm"),
        cancelButtonText: t("common.cancel"),
        inputPattern: /^shutdown$/,
        inputErrorMessage: 'Please type "shutdown" exactly.',
        closeOnClickModal: false,
        closeOnPressEscape: false,
      }
    ).then((result) => result.value).catch(() => "");

    if (typed !== "shutdown") {
      ElMessage.info(t("common.cancelled"));
      return;
    }
  }

  await call(method);
}

async function callQualityBaseline() {
  try {
    const healthRaw = await invokeRuntimeRpc("runtime.health", "{}");
    const traceRaw = await invokeRuntimeRpc("trace.metrics", "{}");
    const metrics = await getMetrics();

    const health = parseRpcOutput(healthRaw)?.result ?? {};
    const trace = parseRpcOutput(traceRaw)?.result ?? {};
    const timeouts = trace?.timeouts ?? {};

    output.value = JSON.stringify(
      {
        quality_baseline: {
          health: {
            is_healthy: Boolean(health?.lifecycle?.is_healthy ?? false),
            shutting_down: Boolean(health?.lifecycle?.shutting_down ?? false),
          },
          metrics: {
            total_requests: Number(metrics?.total_requests ?? 0),
            successful_requests: Number(metrics?.successful_requests ?? 0),
            failed_requests: Number(metrics?.failed_requests ?? 0),
            avg_request_duration_ms: Number(metrics?.avg_request_duration_ms ?? 0),
          },
          benchmark: {
            buffered_events: Number(trace?.buffered_events ?? 0),
            slow_requests_top_n: Array.isArray(trace?.slow_requests_top_n)
              ? trace.slow_requests_top_n.length
              : 0,
            agent_request_timeout_total: Number(timeouts?.agent_request_total ?? 0),
            review_gate_timeout_total: Number(timeouts?.review_gate_total ?? 0),
            runtime_probe_timeout_total: Number(timeouts?.runtime_probe_total ?? 0),
          },
        },
      },
      null,
      2
    );
    ElMessage.success(t("backendOps.done"));
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}
</script>
