<template>
  <el-card>
    <template #header>{{ t("providers.title") }}</template>
    <el-space direction="vertical" alignment="start" style="width:100%">
      <el-text>
        {{ t("providers.hint1") }}
        {{ t("providers.hint2") }}
      </el-text>
      <el-text>
        {{ t("providers.hint3") }}
      </el-text>

      <el-form label-width="140px" style="width:100%;max-width:760px;">
        <el-form-item :label="t('providers.provider')">
          <el-select v-model="provider" style="width:280px" filterable :loading="loadingProviders">
            <el-option
              v-for="item in providerOptions"
              :key="item.name"
              :label="item.name"
              :value="item.name"
            />
          </el-select>
        </el-form-item>
        <el-form-item :label="t('providers.defaultModel')">
          <el-select v-model="selectedModel" style="width:320px" filterable :loading="loadingModels">
            <el-option
              v-for="item in modelOptions"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item :label="t('providers.envVar')">
          <el-input v-model="envVar" :placeholder="t('providers.envVarHint')" />
        </el-form-item>
        <el-form-item :label="t('providers.apiKey')">
          <el-input v-model="apiKey" show-password :placeholder="t('providers.apiKeyHint')" />
        </el-form-item>
        <el-form-item>
          <el-space>
            <el-button type="success" @click="applySelection">{{ t("providers.applySelection") }}</el-button>
            <el-button type="primary" @click="saveApiKey">{{ t("providers.saveApiKey") }}</el-button>
            <el-button type="warning" @click="clearApiKey">{{ t("providers.clearApiKey") }}</el-button>
            <el-button @click="importCopilotToken">{{ t("providers.importCopilot") }}</el-button>
          </el-space>
        </el-form-item>
      </el-form>

      <el-alert v-if="copilotInfo" :title="copilotInfo" type="info" show-icon :closable="false" />
      <el-input v-model="output" type="textarea" :rows="8" readonly />
    </el-space>
  </el-card>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import {
  clearProviderApiKey,
  fetchGithubCopilotToken,
  invokeRuntimeRpc,
  listProviderCatalog,
  ProviderCatalogEntry,
  saveProviderSelection,
  setProviderApiKey,
} from "../services/bridge";

const { t } = useI18n();
const provider = ref("openai");
const envVar = ref("");
const apiKey = ref("");
const output = ref("");
const selectedModel = ref("auto");
const tokenSource = ref("");
const tokenMasked = ref("");
const loadingProviders = ref(false);
const loadingModels = ref(false);
const providerOptions = ref<ProviderCatalogEntry[]>([]);
const modelOptions = ref<Array<{ label: string; value: string }>>([]);

const copilotInfo = computed(() => {
  if (!tokenSource.value) {
    return "";
  }
  return `${t("providers.copilotFound")} ${tokenSource.value} ${tokenMasked.value}`;
});

const selectedProviderSpec = computed(() =>
  providerOptions.value.find((item) => item.name === provider.value)
);

function inferEnvVar(providerName: string) {
  return `${providerName.trim().toUpperCase().replace(/[-\s]+/g, "_")}_API_KEY`;
}

function modelIdFromRuntime(value: unknown): string | undefined {
  if (typeof value === "string" && value.trim()) {
    return value.trim();
  }
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const candidates = [record.id, record.model_id, record.modelId, record.name];
  return candidates.find((item): item is string => typeof item === "string" && item.trim().length > 0)?.trim();
}

function syncEnvVar(previousProvider?: string) {
  const spec = selectedProviderSpec.value;
  const previousSpec = providerOptions.value.find((item) => item.name === previousProvider);
  const previousDefault = previousSpec?.configuredEnvVar || previousSpec?.apiKeyEnv || inferEnvVar(previousProvider || provider.value);
  const nextDefault = spec?.configuredEnvVar || spec?.apiKeyEnv || inferEnvVar(provider.value);
  if (!envVar.value || envVar.value === previousDefault) {
    envVar.value = nextDefault;
  }
}

async function reloadProviderCatalog() {
  loadingProviders.value = true;
  try {
    providerOptions.value = await listProviderCatalog();
    if (!providerOptions.value.some((item) => item.name === provider.value) && providerOptions.value.length > 0) {
      provider.value = providerOptions.value[0].name;
    }
    syncEnvVar();
  } finally {
    loadingProviders.value = false;
  }
}

async function reloadModels() {
  loadingModels.value = true;
  try {
    const options = new Map<string, { label: string; value: string }>();
    options.set("auto", { label: t("providers.autoModel"), value: "auto" });

    const spec = selectedProviderSpec.value;
    if (spec?.defaultModel) {
      options.set(spec.defaultModel, { label: spec.defaultModel, value: spec.defaultModel });
    }

    try {
      const raw = await invokeRuntimeRpc("models/list", "{}");
      const parsed = JSON.parse(raw);
      const payload = typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
      const result = typeof payload.result === "object" && payload.result !== null ? (payload.result as Record<string, unknown>) : payload;
      const groups = Array.isArray(result.models) ? result.models : [];
      const group = groups.find((item) => {
        const record = typeof item === "object" && item !== null ? (item as Record<string, unknown>) : {};
        return record.agent === provider.value;
      }) as Record<string, unknown> | undefined;

      if (group) {
        const defaultModel = modelIdFromRuntime(group.default_model);
        if (defaultModel) {
          options.set(defaultModel, { label: defaultModel, value: defaultModel });
        }
        const models = Array.isArray(group.models) ? group.models : [];
        for (const item of models) {
          const modelId = modelIdFromRuntime(item);
          if (modelId) {
            options.set(modelId, { label: modelId, value: modelId });
          }
        }
      }
    } catch {
      // Fallback to catalog-only model choices when runtime RPC is unavailable.
    }

    modelOptions.value = Array.from(options.values());

    const configuredModel = spec?.configuredModel;
    const nextModel = configuredModel || spec?.defaultModel || "auto";
    if (!modelOptions.value.some((item) => item.value === selectedModel.value)) {
      selectedModel.value = nextModel;
    }
  } finally {
    loadingModels.value = false;
  }
}

async function applySelection() {
  try {
    const result = await saveProviderSelection(provider.value, selectedModel.value, envVar.value || undefined);
    output.value = result.note;
    ElMessage.success(t("providers.selectionSaved"));
    await reloadProviderCatalog();
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}

async function saveApiKey() {
  try {
    output.value = await setProviderApiKey(provider.value, apiKey.value, envVar.value || undefined);
    ElMessage.success(t("providers.saved"));
    apiKey.value = "";
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}

async function clearApiKey() {
  try {
    output.value = await clearProviderApiKey(provider.value, envVar.value || undefined);
    ElMessage.success(t("providers.cleared"));
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}

async function importCopilotToken() {
  try {
    const result = await fetchGithubCopilotToken();
    tokenSource.value = result.source;
    tokenMasked.value = result.tokenMasked || "";
    output.value = result.note;
    if (result.found && result.tokenPlain) {
      provider.value = "copilot";
      envVar.value = "GITHUB_COPILOT_TOKEN";
      apiKey.value = result.tokenPlain;
      if (modelOptions.value.some((item) => item.value === "copilot")) {
        selectedModel.value = "copilot";
      }
      ElMessage.success(t("providers.copilotImported"));
    } else if (result.userCode && result.verificationUri) {
      provider.value = "copilot";
      envVar.value = "GITHUB_COPILOT_TOKEN";
      ElMessage.info(result.note);
    } else {
      ElMessage.warning(t("providers.copilotMissing"));
    }
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}

const stopWatchingProvider = watch(
  provider,
  async (nextProvider, previousProvider) => {
    syncEnvVar(previousProvider);
    await reloadModels();
  },
  { flush: "post" }
);

onUnmounted(() => {
  stopWatchingProvider();
});

onMounted(async () => {
  await reloadProviderCatalog();
  await reloadModels();
});
</script>
