<template>
  <el-dialog
    :model-value="modelValue"
    width="880px"
    top="6vh"
    :close-on-click-modal="false"
    @close="dismissGuide"
  >
    <template #header>
      <div class="guide-header">
        <div>
          <div class="guide-title">{{ t("onboarding.title") }}</div>
          <div class="guide-subtitle">{{ t("onboarding.subtitle") }}</div>
        </div>
        <el-tag :type="runtimeRunning ? 'success' : 'warning'" size="small">
          {{ runtimeRunning ? t("onboarding.runtimeReady") : t("onboarding.runtimeNotReady") }}
        </el-tag>
      </div>
    </template>

    <el-steps :active="activeStep" finish-status="success" simple>
      <el-step :title="t('onboarding.steps.welcome.title')" />
      <el-step :title="t('onboarding.steps.connect.title')" />
      <el-step :title="t('onboarding.steps.workflow.title')" />
      <el-step :title="t('onboarding.steps.finish.title')" />
    </el-steps>

    <div class="guide-body">
      <template v-if="activeStep === 0">
        <el-alert :title="t('onboarding.steps.welcome.tip')" type="info" :closable="false" show-icon />
        <div class="card-grid">
          <el-card shadow="hover" class="guide-card">
            <template #header>{{ t("onboarding.cards.monitor.title") }}</template>
            <p>{{ t("onboarding.cards.monitor.body") }}</p>
            <el-button text type="primary" @click="openTab('monitor', 'dashboard')">
              {{ t("onboarding.actions.openMonitor") }}
            </el-button>
          </el-card>
          <el-card shadow="hover" class="guide-card">
            <template #header>{{ t("onboarding.cards.config.title") }}</template>
            <p>{{ t("onboarding.cards.config.body") }}</p>
            <el-button text type="primary" @click="openTab('config', 'setup')">
              {{ t("onboarding.actions.openSetup") }}
            </el-button>
          </el-card>
          <el-card shadow="hover" class="guide-card">
            <template #header>{{ t("onboarding.cards.chat.title") }}</template>
            <p>{{ t("onboarding.cards.chat.body") }}</p>
            <el-button text type="primary" @click="openTab('chat')">
              {{ t("onboarding.actions.openChat") }}
            </el-button>
          </el-card>
        </div>
      </template>

      <template v-else-if="activeStep === 1">
        <el-alert
          :title="runtimeRunning ? t('onboarding.steps.connect.ready') : t('onboarding.steps.connect.notReady')"
          :type="runtimeRunning ? 'success' : 'warning'"
          :closable="false"
          show-icon
        />
        <div class="action-row">
          <el-button type="primary" @click="$emit('startService')">
            {{ t("onboarding.actions.startRuntime") }}
          </el-button>
          <el-button @click="openTab('config', 'config')">{{ t("onboarding.actions.openConfig") }}</el-button>
          <el-button @click="openTab('config', 'setup')">{{ t("onboarding.actions.runSetup") }}</el-button>
        </div>
        <el-card shadow="never" class="guide-checklist">
          <div class="guide-checklist-title">{{ t("onboarding.checklist.title") }}</div>
          <ul>
            <li>{{ t("onboarding.checklist.executable") }}</li>
            <li>{{ t("onboarding.checklist.protocol") }}</li>
            <li>{{ t("onboarding.checklist.health") }}</li>
          </ul>
        </el-card>
      </template>

      <template v-else-if="activeStep === 2">
        <div class="card-grid">
          <el-card shadow="hover" class="guide-card">
            <template #header>{{ t("onboarding.steps.workflow.providersTitle") }}</template>
            <p>{{ t("onboarding.steps.workflow.providersBody") }}</p>
            <el-button text type="primary" @click="openTab('config', 'providers')">
              {{ t("onboarding.actions.openProviders") }}
            </el-button>
          </el-card>
          <el-card shadow="hover" class="guide-card">
            <template #header>{{ t("onboarding.steps.workflow.observeTitle") }}</template>
            <p>{{ t("onboarding.steps.workflow.observeBody") }}</p>
            <el-button text type="primary" @click="openTab('monitor', 'health')">
              {{ t("onboarding.actions.openMonitor") }}
            </el-button>
          </el-card>
          <el-card shadow="hover" class="guide-card">
            <template #header>{{ t("onboarding.steps.workflow.chatTitle") }}</template>
            <p>{{ t("onboarding.steps.workflow.chatBody") }}</p>
            <el-button text type="primary" @click="openTab('chat')">
              {{ t("onboarding.actions.openChat") }}
            </el-button>
          </el-card>
        </div>
      </template>

      <template v-else>
        <el-result icon="success" :title="t('onboarding.steps.finish.title')" :sub-title="t('onboarding.steps.finish.body')">
          <template #extra>
            <div class="action-row centered">
              <el-button @click="openTab('monitor', 'dashboard')">{{ t("onboarding.actions.openMonitor") }}</el-button>
              <el-button type="primary" @click="completeGuide">{{ t("onboarding.actions.complete") }}</el-button>
            </div>
          </template>
        </el-result>
      </template>
    </div>

    <template #footer>
      <div class="guide-footer">
        <el-button @click="dismissGuide">{{ t("onboarding.actions.later") }}</el-button>
        <div class="guide-footer-right">
          <el-button :disabled="activeStep === 0" @click="activeStep -= 1">{{ t("onboarding.actions.previous") }}</el-button>
          <el-button v-if="activeStep < 3" type="primary" @click="activeStep += 1">{{ t("onboarding.actions.next") }}</el-button>
        </div>
      </div>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { useI18n } from "vue-i18n";

type MainTab = "monitor" | "config" | "chat";
type MonitorSubTab = "dashboard" | "monitor" | "ai-usage" | "health" | "logs";
type ConfigSubTab = "setup" | "config" | "providers" | "backend-ops" | "autotune" | "workflow" | "security";

const props = defineProps<{
  modelValue: boolean;
  runtimeRunning: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  complete: [];
  startService: [];
  navigate: [payload: { mainTab: MainTab; subTab?: MonitorSubTab | ConfigSubTab }];
}>();

const { t } = useI18n();
const activeStep = ref(0);

function openTab(mainTab: MainTab, subTab?: MonitorSubTab | ConfigSubTab) {
  emit("navigate", { mainTab, subTab });
}

function dismissGuide() {
  emit("update:modelValue", false);
}

function completeGuide() {
  emit("complete");
  emit("update:modelValue", false);
}
</script>

<style scoped>
.guide-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.guide-title {
  font-size: 20px;
  font-weight: 700;
}

.guide-subtitle {
  margin-top: 4px;
  color: var(--color-text-secondary, #6b7280);
}

.guide-body {
  min-height: 360px;
  padding: 20px 0 8px;
}

.card-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 16px;
  margin-top: 16px;
}

.guide-card {
  min-height: 180px;
}

.guide-card p {
  margin: 0 0 12px;
  line-height: 1.6;
  color: var(--color-text-secondary, #4b5563);
}

.action-row {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin: 16px 0;
}

.action-row.centered {
  justify-content: center;
}

.guide-checklist {
  margin-top: 8px;
}

.guide-checklist-title {
  font-weight: 600;
  margin-bottom: 8px;
}

.guide-checklist ul {
  margin: 0;
  padding-left: 18px;
  color: var(--color-text-secondary, #4b5563);
  line-height: 1.8;
}

.guide-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.guide-footer-right {
  display: flex;
  gap: 8px;
}

@media (max-width: 900px) {
  .card-grid {
    grid-template-columns: 1fr;
  }
}
</style>