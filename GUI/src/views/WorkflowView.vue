<template>
  <el-space direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>{{ t("workflow.title") }}</template>

      <el-space direction="vertical" fill style="width: 100%">
        <el-text>{{ t("workflow.hint") }}</el-text>

        <!-- 任务列表 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("workflow.availableTasks") }}</span>
              <el-button size="small" :loading="loadingTasks" @click="loadTasks">
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
                <el-button size="small" @click="planTask(row.id)">{{ t("workflow.plan") }}</el-button>
                <el-button
                  size="small"
                  type="primary"
                  :loading="executingId === row.id"
                  @click="executeTask(row.id)"
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
            <el-button type="primary" @click="confirmExecutePlan" :loading="executingPlan">
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
import { ref, reactive } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { invokeRuntimeRpc } from "../services/bridge";

const { t } = useI18n();
const loadingTasks = ref(false);
const executingId = ref("");
const executingPlan = ref(false);

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

const planOutput = ref("");
const rawOutput = ref("");
const plannedTask = ref("");
const activeWorkflows = ref(0);
const totalExecuted = ref(3);
const successCount = ref(2);
const failureCount = ref(1);

function taskTextById(taskId: string) {
  const task = tasks.value.find((item) => item.id === taskId);
  if (!task) {
    return taskId;
  }
  return `${task.name}: ${task.description}`;
}

async function loadTasks() {
  loadingTasks.value = true;
  try {
    const result = await invokeRuntimeRpc("debug_panel.get", "{}");
    rawOutput.value = result;

    const data = JSON.parse(result);
    if (data?.panel) {
      const conversations = data.panel.conversations;
      if (conversations) {
        activeWorkflows.value = Number(conversations.count || 0);
        totalExecuted.value = Number(conversations.checkpoints || totalExecuted.value);
      }
    }
    ElMessage.success(t("common.loaded"));
  } catch (err) {
    ElMessage.error(`Error: ${err}`);
  } finally {
    loadingTasks.value = false;
  }
}

async function planTask(taskId: string) {
  try {
    const task = taskTextById(taskId);
    plannedTask.value = task;
    const params = JSON.stringify({ task });
    const result = await invokeRuntimeRpc("task.plan", params);
    planOutput.value = result;
    ElMessage.success(t("workflow.planGenerated"));
  } catch (err) {
    ElMessage.error(`Error: ${err}`);
  }
}

async function executeTask(taskId: string) {
  executingId.value = taskId;
  try {
    const task = taskTextById(taskId);
    const params = JSON.stringify({ task, requirement_confirmed: true });
    const result = await invokeRuntimeRpc("task.execute", params);
    rawOutput.value = result;

    const data = JSON.parse(result);
    if (data.ok) {
      ElMessage.success(t("workflow.taskStarted"));
      // Add to history
      const task = tasks.value.find((t) => t.id === taskId);
      if (task) {
        executionHistory.value.unshift({
          time: new Date().toLocaleString(),
          task_id: taskId,
          task_name: task.name,
          status: "running",
          duration: "-",
          result: "Executing...",
        });
      }
    }
  } catch (err) {
    ElMessage.error(`Error: ${err}`);
  } finally {
    executingId.value = "";
  }
}

async function confirmExecutePlan() {
  if (!planOutput.value) {
    ElMessage.warning(t("workflow.noPlan"));
    return;
  }

  executingPlan.value = true;
  try {
    const task = plannedTask.value || tasks.value[0]?.name || "workflow task";
    const params = JSON.stringify({
      task,
      requirement_confirmed: true,
      plan: planOutput.value,
    });
    const result = await invokeRuntimeRpc("workflow.execute", params);
    rawOutput.value = result;

    const data = JSON.parse(result);
    if (data.ok) {
      ElMessage.success(t("workflow.workflowStarted"));
      planOutput.value = "";
    }
  } catch (err) {
    ElMessage.error(`Error: ${err}`);
  } finally {
    executingPlan.value = false;
  }
}

// Initialize
loadTasks();
</script>
