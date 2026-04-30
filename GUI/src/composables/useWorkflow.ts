import { ref, reactive, type Ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { invokeRuntimeRpc } from "../services/bridge";

export interface TimelineEntry {
  iteration: number;
  status: string;
  next_action: string;
  patch_set_size: number;
  test_gate_result: string;
}

export interface ExecutionInsights {
  latestRunMode: Ref<string>;
  latestGates: Ref<Record<string, unknown>>;
  latestAutoRepair: Ref<Record<string, unknown>>;
  latestCycleTimeline: Ref<TimelineEntry[]>;
}

export function useWorkflow() {
  const { t } = useI18n();

  const loadingTasks = ref(false);
  const executingId = ref("");
  const executingPlan = ref(false);
  const loadingPeak = ref(false);

  const planOutput = ref("");
  const rawOutput = ref("");
  const plannedTask = ref("");
  const activeWorkflows = ref(0);
  const totalExecuted = ref(3);
  const successCount = ref(2);
  const failureCount = ref(1);

  const latestCycleTimeline = ref<TimelineEntry[]>([]);
  const latestGates = ref<Record<string, unknown>>({});
  const latestAutoRepair = ref<Record<string, unknown>>({});
  const latestRunMode = ref("assisted");

  const peakIndicators = reactive({
    taskSuccessRate: 0,
    firstPassRate: 0,
    meanRepairIterations: 0,
    humanInterventionRate: 0,
    regressionRate: 0,
  });

  function hydrateExecutionInsights(payload: Record<string, unknown>) {
    const result = (payload?.result ?? payload ?? {}) as Record<
      string,
      unknown
    >;
    latestRunMode.value = String(result?.run_mode || "assisted");
    latestGates.value = (result?.gates || {}) as Record<string, unknown>;

    const cycle = (result?.execution_cycle || {}) as Record<string, unknown>;
    latestAutoRepair.value = (cycle?.auto_repair || {}) as Record<
      string,
      unknown
    >;
    const cycles = Array.isArray(cycle?.cycles) ? cycle.cycles : [];
    latestCycleTimeline.value = cycles.map((item: Record<string, unknown>) => ({
      iteration: Number(item?.iteration || 0),
      status: String(item?.status || "unknown"),
      next_action: String(item?.next_action || "-"),
      patch_set_size: Number(item?.patch_set_size || 0),
      test_gate_result: String(item?.test_gate_result || "not_run"),
    }));
  }

  function requirementGateStatus(): string {
    const gate = latestGates.value as Record<string, unknown>;
    return String(
      (gate?.requirement as Record<string, unknown>)?.status ||
        gate?.gate ||
        "-",
    );
  }

  function reviewGateStatus(): string {
    const gate = latestGates.value as Record<string, unknown>;
    return String(gate?.status2 || "-");
  }

  function repairStatusText(): string {
    const repair = latestAutoRepair.value as Record<string, unknown>;
    return String(repair?.status || "-");
  }

  function repairTargetCount(): number {
    const repair = latestAutoRepair.value as Record<string, unknown>;
    const targets = Array.isArray(repair?.target_subtasks)
      ? repair.target_subtasks
      : [];
    return Number(targets.length || 0);
  }

  async function refreshPeakIndicators() {
    loadingPeak.value = true;
    try {
      const result = await invokeRuntimeRpc("optimization.peak", "{}");
      const payload = JSON.parse(result);
      const indicators = payload?.result?.peak?.indicators || {};
      peakIndicators.taskSuccessRate = Number(
        indicators.task_success_rate || 0,
      );
      peakIndicators.firstPassRate = Number(indicators.first_pass_rate || 0);
      peakIndicators.meanRepairIterations = Number(
        indicators.mean_repair_iterations || 0,
      );
      peakIndicators.humanInterventionRate = Number(
        indicators.human_intervention_rate || 0,
      );
      peakIndicators.regressionRate = Number(indicators.regression_rate || 0);
    } catch (err) {
      ElMessage.error(t("workflow.error", { error: err }));
    } finally {
      loadingPeak.value = false;
    }
  }

  function taskTextById(
    taskId: string,
    tasks: Ref<Array<{ id: string; name: string; description: string }>>,
  ): string {
    const task = tasks.value.find((item) => item.id === taskId);
    if (!task) {
      return taskId;
    }
    return `${task.name}: ${task.description}`;
  }

  async function loadTasks(
    tasks: Ref<
      Array<{
        id: string;
        name: string;
        description: string;
        estimated_duration: string;
      }>
    >,
    executionHistory: Ref<
      Array<{
        time: string;
        task_id: string;
        task_name: string;
        status: string;
        duration: string;
        result: string;
      }>
    >,
  ) {
    loadingTasks.value = true;
    try {
      const result = await invokeRuntimeRpc("debug_panel.get", "{}");
      rawOutput.value = result;

      const data = JSON.parse(result);
      if (data?.panel) {
        const conversations = data.panel.conversations;
        if (conversations) {
          activeWorkflows.value = Number(conversations.count || 0);
          totalExecuted.value = Number(
            conversations.checkpoints || totalExecuted.value,
          );
        }

        // Populate tasks from backend if available
        const backendTasks = data.panel.tasks;
        if (Array.isArray(backendTasks) && backendTasks.length > 0) {
          tasks.value = backendTasks.map((t: Record<string, unknown>) => ({
            id: String(t.id || ""),
            name: String(t.name || ""),
            description: String(t.description || ""),
            estimated_duration: String(t.estimated_duration || "-"),
          }));
        }

        // Populate executionHistory from backend if available
        const backendHistory =
          data.panel.execution_history ?? data.panel.history;
        if (Array.isArray(backendHistory) && backendHistory.length > 0) {
          executionHistory.value = backendHistory.map(
            (h: Record<string, unknown>) => ({
              time: String(h.time || h.timestamp || "-"),
              task_id: String(h.task_id || "-"),
              task_name: String(h.task_name || "-"),
              status: String(h.status || "unknown"),
              duration: String(h.duration || "-"),
              result: String(h.result || "-"),
            }),
          );
        }
      }
      ElMessage.success(t("common.loaded"));
    } catch (err) {
      ElMessage.error(t("workflow.error", { error: err }));
    } finally {
      loadingTasks.value = false;
    }
  }

  async function planTask(
    taskId: string,
    tasks: Ref<Array<{ id: string; name: string; description: string }>>,
  ) {
    try {
      const task = taskTextById(taskId, tasks);
      plannedTask.value = task;
      const params = JSON.stringify({ task });
      const result = await invokeRuntimeRpc("task.plan", params);
      planOutput.value = result;
      hydrateExecutionInsights(JSON.parse(result));
      ElMessage.success(t("workflow.planGenerated"));
    } catch (err) {
      ElMessage.error(t("workflow.error", { error: err }));
    }
  }

  async function executeTask(
    taskId: string,
    tasks: Ref<
      Array<{
        id: string;
        name: string;
        description: string;
        estimated_duration: string;
      }>
    >,
    executionHistory: Ref<
      Array<{
        time: string;
        task_id: string;
        task_name: string;
        status: string;
        duration: string;
        result: string;
      }>
    >,
  ) {
    executingId.value = taskId;
    try {
      const task = taskTextById(taskId, tasks);
      const params = JSON.stringify({ task, requirement_confirmed: true });
      const result = await invokeRuntimeRpc("task.execute", params);
      rawOutput.value = result;

      const data = JSON.parse(result);
      hydrateExecutionInsights(data);
      if (data.ok) {
        ElMessage.success(t("workflow.taskStarted"));
        const found = tasks.value.find((t) => t.id === taskId);
        if (found) {
          executionHistory.value.unshift({
            time: new Date().toLocaleString(),
            task_id: taskId,
            task_name: found.name,
            status: "running",
            duration: "-",
            result: "Executing...",
          });
        }
      }
    } catch (err) {
      ElMessage.error(t("workflow.error", { error: err }));
    } finally {
      executingId.value = "";
    }
  }

  async function confirmExecutePlan(
    tasks: Ref<
      Array<{
        id: string;
        name: string;
        description: string;
        estimated_duration: string;
      }>
    >,
    executionHistory: Ref<
      Array<{
        time: string;
        task_id: string;
        task_name: string;
        status: string;
        duration: string;
        result: string;
      }>
    >,
  ) {
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
      hydrateExecutionInsights(data);
      if (data.ok) {
        ElMessage.success(t("workflow.workflowStarted"));
        planOutput.value = "";
      }
    } catch (err) {
      ElMessage.error(t("workflow.error", { error: err }));
    } finally {
      executingPlan.value = false;
    }
  }

  return {
    // State
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
    latestGates,
    latestAutoRepair,
    latestRunMode,
    peakIndicators,

    // Methods
    hydrateExecutionInsights,
    requirementGateStatus,
    reviewGateStatus,
    repairStatusText,
    repairTargetCount,
    refreshPeakIndicators,
    loadTasks,
    planTask,
    executeTask,
    confirmExecutePlan,
  };
}
