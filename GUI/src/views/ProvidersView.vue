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
          <el-select v-model="provider" style="width:240px" filterable allow-create default-first-option>
            <el-option label="openai" value="openai" />
            <el-option label="anthropic" value="anthropic" />
            <el-option label="gemini" value="gemini" />
            <el-option label="copilot" value="copilot" />
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
import { computed, ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { clearProviderApiKey, fetchGithubCopilotToken, setProviderApiKey } from "../services/bridge";

const { t } = useI18n();
const provider = ref("openai");
const envVar = ref("");
const apiKey = ref("");
const output = ref("");
const tokenSource = ref("");
const tokenMasked = ref("");

const copilotInfo = computed(() => {
  if (!tokenSource.value) {
    return "";
  }
  return `${t("providers.copilotFound")} ${tokenSource.value} ${tokenMasked.value}`;
});

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
      ElMessage.success(t("providers.copilotImported"));
    } else {
      ElMessage.warning(t("providers.copilotMissing"));
    }
  } catch (error) {
    output.value = String(error);
    ElMessage.error(String(error));
  }
}
</script>
