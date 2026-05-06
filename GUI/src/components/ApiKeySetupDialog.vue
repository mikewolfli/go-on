<template>
  <el-dialog
    v-model="visible"
    :title="t('apiKeySetup.title')"
    :close-on-click-modal="false"
    :width="480"
    top="15vh"
    @closed="handleClosed"
  >
    <template v-if="!completed">
      <el-alert
        v-if="errorMsg"
        :title="errorMsg"
        type="error"
        show-icon
        :closable="false"
        style="margin-bottom: 16px"
      />
      <el-form label-position="top" style="width: 100%">
        <el-form-item :label="t('apiKeySetup.provider')">
          <el-select
            v-model="provider"
            style="width: 100%"
            filterable
            :loading="loadingProviders"
          >
            <el-option
              v-for="item in providerOptions"
              :key="item.name"
              :label="item.name"
              :value="item.name"
            />
          </el-select>
        </el-form-item>
        <el-form-item :label="t('apiKeySetup.apiKey')">
          <el-input
            v-model="apiKey"
            show-password
            :placeholder="t('apiKeySetup.apiKeyPlaceholder')"
            autocomplete="new-password"
          />
        </el-form-item>
        <el-form-item :label="t('apiKeySetup.envVar')">
          <el-input v-model="envVar" :placeholder="t('apiKeySetup.envVarHint')" />
        </el-form-item>
        <el-form-item :label="t('apiKeySetup.model')">
          <el-select v-model="selectedModel" style="width: 100%" filterable :loading="loadingModels">
            <el-option
              v-for="item in modelOptions"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
      </el-form>
    </template>
    <template v-else>
      <el-result
        icon="success"
        :title="t('apiKeySetup.completedTitle')"
        :sub-title="t('apiKeySetup.completedSubtitle')"
      />
    </template>
    <template #footer>
      <el-button v-if="!completed" @click="visible = false">
        {{ t('apiKeySetup.skip') }}
      </el-button>
      <el-button v-if="!completed" type="primary" :loading="saving" @click="handleSave">
        {{ t('apiKeySetup.save') }}
      </el-button>
      <el-button v-else type="primary" @click="visible = false">
        {{ t('apiKeySetup.gotIt') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import {
  listProviderCatalog,
  ProviderCatalogEntry,
  saveProviderSelection,
  setProviderApiKey,
  invokeRuntimeRpc,
} from "../services/bridge";
import { useRuntimeStore } from "../stores/runtime";

let modelLoadGeneration = 0;

const { t } = useI18n();
const runtime = useRuntimeStore();

const visible = defineModel<boolean>("visible", { default: false });
const emit = defineEmits<{
  configured: [];
}>();

const provider = ref("");
const apiKey = ref("");
const envVar = ref("");
const selectedModel = ref("auto");
const saving = ref(false);
const completed = ref(false);
const errorMsg = ref("");
const loadingProviders = ref(false);
const loadingModels = ref(false);
const providerOptions = ref<ProviderCatalogEntry[]>([]);
const modelOptions = ref<Array<{ label: string; value: string }>>([]);

function reset() {
  provider.value = "";
  apiKey.value = "";
  envVar.value = "";
  selectedModel.value = "auto";
  saving.value = false;
  completed.value = false;
  errorMsg.value = "";
}

function inferEnvVar(providerName: string) {
  return `${providerName.trim().toUpperCase().replace(/[-\s]+/g, "_")}_API_KEY`;
}

function modelIdFromRuntime(value: unknown): string | undefined {
  if (typeof value === "string" && value.trim()) return value.trim();
  if (typeof value !== "object" || value === null) return undefined;
  const record = value as Record<string, unknown>;
  const candidates = [record.id, record.model_id, record.modelId, record.name];
  return candidates.find((item): item is string => typeof item === "string" && item.trim().length > 0)?.trim();
}

async function loadProviders() {
  loadingProviders.value = true;
  try {
    providerOptions.value = await listProviderCatalog();
    if (providerOptions.value.length > 0 && !provider.value) {
      provider.value = providerOptions.value[0].name;
      syncEnvVar();
    }
  } finally {
    loadingProviders.value = false;
  }
}

async function loadModels() {
  const gen = ++modelLoadGeneration;
  loadingModels.value = true;
  try {
    if (gen !== modelLoadGeneration) return;
    const options = new Map<string, { label: string; value: string }>();
    options.set("auto", { label: t("apiKeySetup.autoModel"), value: "auto" });

    const spec = providerOptions.value.find((item) => item.name === provider.value);
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
        if (defaultModel) options.set(defaultModel, { label: defaultModel, value: defaultModel });
        const models = Array.isArray(group.models) ? group.models : [];
        for (const item of models) {
          const modelId = modelIdFromRuntime(item);
          if (modelId) options.set(modelId, { label: modelId, value: modelId });
        }
      }
    } catch {
      // runtime RPC unavailable, use catalog-only models
    }

    if (gen !== modelLoadGeneration) return;
    modelOptions.value = Array.from(options.values());
    const nextModel = spec?.configuredModel || spec?.defaultModel || "auto";
    if (!modelOptions.value.some((item) => item.value === selectedModel.value)) {
      selectedModel.value = nextModel;
    }
  } finally {
    if (gen === modelLoadGeneration) loadingModels.value = false;
  }
}

function syncEnvVar() {
  const spec = providerOptions.value.find((item) => item.name === provider.value);
  envVar.value = spec?.configuredEnvVar || spec?.apiKeyEnv || inferEnvVar(provider.value);
}

watch(provider, async () => {
  errorMsg.value = '';
  syncEnvVar();
  await loadModels();
});

async function handleSave() {
  if (!provider.value) {
    errorMsg.value = t("apiKeySetup.errorNoProvider");
    return;
  }
  if (!apiKey.value || apiKey.value.trim().length < 4) {
    errorMsg.value = t("apiKeySetup.errorInvalidKey");
    return;
  }

  saving.value = true;
  errorMsg.value = "";
  try {
    // Step 1: save provider selection to config.toml
    await saveProviderSelection(provider.value, selectedModel.value, envVar.value || undefined);

    // Step 2: save API key
    await setProviderApiKey(provider.value, apiKey.value.trim(), envVar.value || undefined);

    // Step 3: refresh all states
    await runtime.refreshAll();

    // Step 4: verify readiness
    try {
      const { getProviderStatus } = await import("../services/rpcService");
      const status = await getProviderStatus();
      const agents = status?.provider_status?.configured_agents || [];
      const ready = agents.some((p: any) => p?.ready === true);
      if (!ready) {
        errorMsg.value = t("apiKeySetup.errorNotReady");
        saving.value = false;
        return;
      }
    } catch {
      // verification RPC failed, still mark as done
    }

    completed.value = true;
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    saving.value = false;
  }
}

function handleClosed() {
  if (completed.value) {
    emit("configured");
  }
}

onMounted(async () => {
  reset();
  await loadProviders();
  await loadModels();
});
</script>
