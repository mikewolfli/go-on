<template>
  <el-space direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>{{ t("workflow.title") }}</template>

      <el-space direction="vertical" fill style="width: 100%">
        <el-text>{{ t("workflow.hint") }}</el-text>

        <!-- Demo data warning -->
        <el-alert
          v-if="isUsingDemoData"
          :title="t('common.offlineMode') || 'Backend Offline'"
          :description="t('workflow.demoDataWarning') || 'Displaying placeholder demo data. Start the backend to see live workflow data.'"
          type="warning"
          show-icon
          :closable="false"
        />

        <!-- 任务列表 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("workflow.availableTasks") }}</span>
              <el-button size="small" :loading="loadingTasks" @click="onLoadTasks">
                {{ t("common.refresh") }}
              </el-button>
            </div>
          </template>
          <el-table :data="tasks" border stripe>
            <el-table-column prop="id" :label="t('common.id')" width="100" />
            <el-table-column prop="name" :label="t('common.name')" width="200" />
            <el-table-column prop="description" :label="t('common.description')" />
            <el-table-column prop="estimated_duration" :label="t('workflow.estimatedDuration')" width="120" />
            <el-table-column :label="t('common.action')" width="150">
              <template #default="{ row }">
                <el-button size="small" @click="onPlanTask(row.id)">{{ t("workflow.plan") }}</el-button>
                <el-button
                  size="small"
                  type="primary"
                  :loading="executingId === row.id"
                  @click="onExecuteTask(row.id)"
                >
                  {{ t("workflow.execute") }}
                </el-button>
              </template>
            </el-table-column>
          </el-table>
        </el-card>

        <!-- 执行计划 -->
        <el-card shadow="hover" v-if="planOutput">
          <template #header>
            <span>{{ t("workflow.executionPlan") }}</span>
          </template>
          <el-space direction="vertical" fill style="width: 100%">
            <el-input v-model="planOutput" type="textarea" :rows="8" readonly />
            <el-button type="primary" @click="onConfirmExecutePlan" :loading="executingPlan">
              {{ t("workflow.confirmExecute") }}
            </el-button>
          </el-space>
        </el-card>

        <!-- 执行历史 -->
        <el-card shadow="hover">
          <template #header>
            <span>{{ t("workflow.history") }}</span>
          </template>
          <el-table :data="executionHistory" border stripe>
            <el-table-column prop="time" :label="t('common.time')" width="180" />
            <el-table-column prop="task_id" :label="t('workflow.taskId')" width="100" />
            <el-table-column prop="task_name" :label="t('common.name')" width="150" />
            <el-table-column prop="status" :label="t('common.status')" width="100">
              <template #default="{ row }">
                <el-tag
                  :type="row.status === 'success' ? 'success' : row.status === 'failed' ? 'danger' : 'info'"
                >
                  {{ row.status }}
                </el-tag>
              </template>
            </el-table-column>
            <el-table-column prop="duration" :label="t('workflow.duration')" width="100" />
            <el-table-column prop="result" :label="t('common.result')" />
          </el-table>
        </el-card>

        <!-- 当前状态 -->
        <el-card shadow="hover">
          <template #header>
            <span>{{ t("workflow.currentStatus") }}</span>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('workflow.activeWorkflows')">
              {{ activeWorkflows }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.totalExecuted')">
              {{ totalExecuted }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.successCount')">
              <el-tag type="success">{{ successCount }}</el-tag>
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.failureCount')">
              <el-tag type="danger">{{ failureCount }}</el-tag>
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
                <span>{{ t('views.WorkflowView.cycleTimeline') }}</span>
                <el-tag size="small" type="info">{{ latestRunMode }}</el-tag>
              </div>
          </template>
          <el-table :data="latestCycleTimeline" border stripe>
            <el-table-column prop="iteration" :label="t('common.name') + ' #'" width="100" />
            <el-table-column prop="status" :label="t('common.status')" width="130" />
            <el-table-column prop="next_action" :label="t('workflow.nextAction') || 'Next Action'" />
            <el-table-column prop="patch_set_size" :label="t('workflow.patchSet') || 'Patch Set'" width="120" />
            <el-table-column prop="test_gate_result" :label="t('workflow.gate') || 'Gate'" width="120" />
          </el-table>
        </el-card>

        <el-card shadow="hover">
          <template #header>
            <span>{{ t('views.WorkflowView.gateMatrix') }}</span>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('workflow.requirementGate') || 'Requirement Gate'">
              {{ requirementGateStatus() }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.reviewGate') || 'Review Gate'">
              {{ reviewGateStatus() }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.repairStatus') || 'Repair Status'">
              {{ repairStatusText() }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.repairTargets') || 'Repair Targets'">
              {{ repairTargetCount() }}
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t('views.WorkflowView.benchmarkIndicators') }}</span>
              <el-button size="small" :loading="loadingPeak" @click="refreshPeakIndicators">{{ t('common.refresh') }}</el-button>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('workflow.taskSuccessRate') || 'Task Success Rate'">
              {{ (peakIndicators.taskSuccessRate ?? 0).toFixed(4) }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.firstPassRate') || 'First Pass Rate'">
              {{ (peakIndicators.firstPassRate ?? 0).toFixed(4) }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.meanRepairIterations') || 'Mean Repair Iterations'">
              {{ (peakIndicators.meanRepairIterations ?? 0).toFixed(4) }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.humanInterventionRate') || 'Human Intervention Rate'">
              {{ (peakIndicators.humanInterventionRate ?? 0).toFixed(4) }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('workflow.regressionRate') || 'Regression Rate'">
              {{ (peakIndicators.regressionRate ?? 0).toFixed(4) }}
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-divider />

        <!-- Raw Output -->
        <el-collapse>
          <el-collapse-item :title="t('common.advancedInfo')" name="1">
            <el-input v-model="rawOutput" type="textarea" :rows="12" readonly />
          </el-collapse-item>
        </el-collapse>
      </el-space>
    </el-card>
  </el-space>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useWorkflow } from "../composables/useWorkflow";
import { useRuntimeStore } from "../stores/runtime";

const { t } = useI18n();
const runtime = useRuntimeStore();

// Demo/placeholder data indicator
const isUsingDemoData = computed(
  () => !runtime.status.running || runtime.offline,
);

const tasks = ref([
  {
    id: "task_001",
    name: "Health Check Workflow",
    description: "Run comprehensive health checks across all components",
    estimated_duration: "2m",
  },
  {
    id: "task_002",
    name: "Cache Optimization",
    description: "Optimize cache settings based on usage patterns",
    estimated_duration: "5m",
  },
  {
    id: "task_003",
    name: "Vector Rebuild",
    description: "Rebuild vector index for better performance",
    estimated_duration: "10m",
  },
]);

const executionHistory = ref([
  {
    time: "2026-04-13 14:30:45",
    task_id: "task_001",
    task_name: "Health Check Workflow",
    status: "success",
    duration: "2m 15s",
    result: "All checks passed",
  },
  {
    time: "2026-04-13 14:25:30",
    task_id: "task_002",
    task_name: "Cache Optimization",
    status: "success",
    duration: "4m 50s",
    result: "Improved hit rate by 8%",
  },
  {
    time: "2026-04-13 14:10:00",
    task_id: "task_003",
    task_name: "Vector Rebuild",
    status: "failed",
    duration: "8m 30s",
    result: "Timeout exceeded, will retry",
  },
]);

const {
  loadingTasks,
  executingId,
  executingPlan,
  loadingPeak,
  planOutput,
  rawOutput,
  plannedTask,
  activeWorkflows,
  totalExecuted,
  successCount,
  failureCount,
  latestCycleTimeline,
  latestRunMode,
  peakIndicators,

  requirementGateStatus,
  reviewGateStatus,
  repairStatusText,
  repairTargetCount,
  refreshPeakIndicators,
  loadTasks,
  planTask,
  executeTask,
  confirmExecutePlan,
} = useWorkflow();

// Wrappers to pass refs correctly (template auto-unwraps refs to plain arrays)
function onLoadTasks() {
  loadTasks(tasks, executionHistory);
}
function onPlanTask(id: string) {
  planTask(id, tasks);
}
function onExecuteTask(id: string) {
  executeTask(id, tasks, executionHistory);
}
function onConfirmExecutePlan() {
  confirmExecutePlan(tasks, executionHistory);
}

onMounted(() => {
  loadTasks(tasks, executionHistory).catch((err) => {
    if (import.meta.env.DEV) {
      console.warn("WorkflowView: loadTasks failed (backend may be offline)", err);
    }
  });
  refreshPeakIndicators().catch((err) => {
    if (import.meta.env.DEV) {
      console.warn("WorkflowView: refreshPeakIndicators failed (backend may be offline)", err);
    }
  });
});
</script>
