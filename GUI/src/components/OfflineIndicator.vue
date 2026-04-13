<template>
  <transition name="slide-fade">
    <div
      v-if="runtime.offline"
      style="
        background-color: #fff3cd;
        border: 1px solid #ffc107;
        border-radius: 4px;
        padding: 12px 16px;
        margin-bottom: 12px;
        display: flex;
        align-items: center;
        gap: 8px;
      "
    >
      <span style="font-size: 20px">⚠️</span>
      <span style="font-weight: 500">{{ t("common.offlineMode") }}</span>
      <span style="font-size: 12px; color: #666">
        {{ t("common.offlineModeHint") }}
      </span>
    </div>
  </transition>
</template>

<script setup lang="ts">
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";

const runtime = useRuntimeStore();
const { t } = useI18n();
</script>

<style scoped>
.slide-fade-enter-active,
.slide-fade-leave-active {
  transition: all 0.3s ease;
}

.slide-fade-enter-from {
  translate: -30px;
  opacity: 0;
}

.slide-fade-leave-to {
  translate: -30px;
  opacity: 0;
}
</style>
