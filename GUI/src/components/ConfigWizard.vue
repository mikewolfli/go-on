<template>
  <el-dialog
    :model-value="modelValue"
    width="720px"
    :close-on-click-modal="false"
    @close="close"
  >
    <template #header>
      <div class="wizard-header">
        <div class="wizard-title">{{ t("config.wizard.title") }}</div>
        <div class="wizard-subtitle">{{ t("config.wizard.subtitle") }}</div>
      </div>
    </template>

    <el-steps :active="step" finish-status="success" simple>
      <el-step :title="t('config.wizard.step1.title')" />
      <el-step :title="t('config.wizard.step2.title')" />
      <el-step :title="t('config.wizard.step3.title')" />
    </el-steps>

    <div class="wizard-body">
      <template v-if="step === 0">
        <div class="wizard-step-title">{{ t("config.wizard.step1.description") }}</div>
        <div class="scenario-grid">
          <button
            v-for="option in scenarios"
            :key="option.value"
            type="button"
            :class="['scenario-card', option.value === scenario ? 'selected' : '']"
            @click="selectScenario(option.value)"
          >
            <div class="scenario-name">{{ option.label }}</div>
            <div class="scenario-body">{{ option.description }}</div>
          </button>
        </div>
      </template>

      <template v-else-if="step === 1">
        <div class="wizard-step-title">{{ t("config.wizard.step2.description") }}</div>
        <div class="mode-grid">
          <button
            v-for="mode in protocolModes"
            :key="mode.value"
            type="button"
            :class="['mode-card', mode.value === draft.protocolMode ? 'selected' : '']"
            @click="draft.protocolMode = mode.value"
          >
            <div class="mode-head">
              <span>{{ mode.label }}</span>
              <el-tag v-if="mode.recommended" size="small" type="success">{{ t("config.wizard.recommended") }}</el-tag>
            </div>
            <div class="mode-description">{{ mode.description }}</div>
            <div class="mode-note">{{ mode.recommendedFor }}</div>
          </button>
        </div>
      </template>

      <template v-else>
        <div class="wizard-step-title">{{ t("config.wizard.step3.description") }}</div>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('config.executable')">{{ draft.executablePath }}</el-descriptions-item>
          <el-descriptions-item :label="t('config.workingDir')">{{ draft.workingDir }}</el-descriptions-item>
          <el-descriptions-item :label="t('config.protocolMode')">{{ selectedMode?.label }}</el-descriptions-item>
          <el-descriptions-item :label="t('config.monitorOnly')">{{ draft.monitorOnly ? t('config.yes') : t('config.no') }}</el-descriptions-item>
        </el-descriptions>
      </template>
    </div>

    <template #footer>
      <div class="wizard-footer">
        <el-button @click="close">{{ t("config.wizard.cancel") }}</el-button>
        <div>
          <el-button :disabled="step === 0" @click="step -= 1">{{ t("config.wizard.prev") }}</el-button>
          <el-button v-if="step < 2" type="primary" @click="step += 1">{{ t("config.wizard.next") }}</el-button>
          <el-button v-else type="primary" @click="apply">{{ t("config.wizard.finish") }}</el-button>
        </div>
      </div>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

interface ConfigWizardDraft {
  executablePath: string;
  workingDir: string;
  protocolMode: string;
  monitorOnly: boolean;
}

const props = defineProps<{
  modelValue: boolean;
  executablePath: string;
  workingDir: string;
  protocolMode: string;
  monitorOnly: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  apply: [draft: ConfigWizardDraft];
}>();

const { t } = useI18n();
const step = ref(0);
const scenario = ref<"local" | "shared" | "editor">("local");
const draft = reactive<ConfigWizardDraft>({
  executablePath: props.executablePath,
  workingDir: props.workingDir,
  protocolMode: props.protocolMode,
  monitorOnly: props.monitorOnly,
});

watch(
  () => props.modelValue,
  (visible) => {
    if (!visible) {
      return;
    }
    step.value = 0;
    scenario.value = "local";
    draft.executablePath = props.executablePath;
    draft.workingDir = props.workingDir;
    draft.protocolMode = props.protocolMode;
    draft.monitorOnly = props.monitorOnly;
  },
);

const scenarios = computed<Array<{ value: "local" | "shared" | "editor"; label: string; description: string }>>(() => [
  {
    value: "local",
    label: t("config.wizard.scenarios.local.title"),
    description: t("config.wizard.scenarios.local.description"),
  },
  {
    value: "shared",
    label: t("config.wizard.scenarios.shared.title"),
    description: t("config.wizard.scenarios.shared.description"),
  },
  {
    value: "editor",
    label: t("config.wizard.scenarios.editor.title"),
    description: t("config.wizard.scenarios.editor.description"),
  },
]);

const protocolModes = computed(() => [
  {
    value: "from_config",
    label: t("config.protocolModeFromConfig"),
    description: t("config.protocolModeFromConfigDesc"),
    recommendedFor: t("config.protocolModeFromConfigRecommended"),
    recommended: scenario.value === "local",
  },
  {
    value: "adaptive",
    label: "adaptive",
    description: t("protocol_mode.adaptive.description"),
    recommendedFor: t("protocol_mode.adaptive.recommended_for"),
    recommended: scenario.value === "local",
  },
  {
    value: "acp_stdio",
    label: "acp_stdio",
    description: t("protocol_mode.acp_stdio.description"),
    recommendedFor: t("protocol_mode.acp_stdio.recommended_for"),
    recommended: scenario.value === "editor",
  },
  {
    value: "acp_http",
    label: "acp_http",
    description: t("protocol_mode.acp_http.description"),
    recommendedFor: t("protocol_mode.acp_http.recommended_for"),
    recommended: scenario.value === "shared",
  },
  {
    value: "mcp_stdio",
    label: "mcp_stdio",
    description: t("protocol_mode.mcp_stdio.description"),
    recommendedFor: t("protocol_mode.mcp_stdio.recommended_for"),
    recommended: false,
  },
  {
    value: "mcp_http",
    label: "mcp_http",
    description: t("protocol_mode.mcp_http.description"),
    recommendedFor: t("protocol_mode.mcp_http.recommended_for"),
    recommended: false,
  },
]);

const selectedMode = computed(() => protocolModes.value.find((item) => item.value === draft.protocolMode));

function selectScenario(value: "local" | "shared" | "editor") {
  scenario.value = value;
  if (value === "local") {
    draft.protocolMode = "adaptive";
    draft.monitorOnly = false;
  }
  if (value === "shared") {
    draft.protocolMode = "acp_http";
    draft.monitorOnly = true;
  }
  if (value === "editor") {
    draft.protocolMode = "acp_stdio";
    draft.monitorOnly = false;
  }
}

function close() {
  emit("update:modelValue", false);
}

function apply() {
  emit("apply", { ...draft });
  emit("update:modelValue", false);
}
</script>

<style scoped>
.wizard-header {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.wizard-title {
  font-size: 18px;
  font-weight: 700;
}

.wizard-subtitle {
  color: var(--color-text-secondary, #6b7280);
}

.wizard-body {
  padding-top: 18px;
  min-height: 320px;
}

.wizard-step-title {
  margin-bottom: 16px;
  font-weight: 600;
}

.scenario-grid,
.mode-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
}

.mode-grid {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.scenario-card,
.mode-card {
  border: 1px solid var(--color-border, #d1d5db);
  border-radius: 12px;
  background: var(--color-surface, #ffffff);
  padding: 14px;
  text-align: left;
  cursor: pointer;
}

.scenario-card.selected,
.mode-card.selected {
  border-color: var(--color-accent, #3b82f6);
  box-shadow: 0 0 0 1px var(--color-accent, #3b82f6);
}

.scenario-name,
.mode-head {
  font-weight: 600;
  margin-bottom: 8px;
}

.mode-head {
  display: flex;
  justify-content: space-between;
  gap: 8px;
}

.scenario-body,
.mode-description,
.mode-note {
  line-height: 1.6;
  color: var(--color-text-secondary, #4b5563);
}

.mode-note {
  margin-top: 8px;
  font-size: 12px;
}

.wizard-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

@media (max-width: 820px) {
  .scenario-grid,
  .mode-grid {
    grid-template-columns: 1fr;
  }
}
</style>
