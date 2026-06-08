import * as vscode from "vscode";
import { i18n, MessageKeys } from "../i18n";
import {
  asRecord,
  getErrorMessage,
  safeStringify,
  ensureRunning,
  RpcCommandRegistryDeps,
} from "./rpcShared";

/**
 * Register workflow-related RPC commands: execution, task plan/execute,
 * hardness/cost estimation, checkpoints, and debugging.
 */
export function registerWorkflowRpcCommands(
  deps: RpcCommandRegistryDeps,
): vscode.Disposable[] {
  // ── Workflow execution ──────────────────────────────────────────────
  const workflowExecuteRpcCommand = vscode.commands.registerCommand(
    "go-on.workflowExecute",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const objective = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptWorkflowObjective),
        placeHolder: i18n.getMessage(
          MessageKeys.promptWorkflowObjectivePlaceholder,
        ),
      });
      if (!objective) {
        return;
      }
      try {
        const result = await deps.sendRequest("workflow.execute", {
          task: objective,
        });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandCompleted, [
            "workflow.execute",
            safeStringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "workflow.execute",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Task plan ───────────────────────────────────────────────────────
  const taskPlanRpcCommand = vscode.commands.registerCommand(
    "go-on.taskPlan",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const task = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptTaskPlan),
        placeHolder: i18n.getMessage(MessageKeys.promptTaskPlanPlaceholder),
      });
      if (!task) {
        return;
      }
      try {
        const result = await deps.sendRequest("task.plan", { task });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandCompleted, [
            "task.plan",
            safeStringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "task.plan",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Task execute ────────────────────────────────────────────────────
  const taskExecuteRpcCommand = vscode.commands.registerCommand(
    "go-on.taskExecute",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const task = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptTaskExecute),
        placeHolder: i18n.getMessage(MessageKeys.promptTaskExecutePlaceholder),
      });
      if (!task) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("task.execute", {
            task,
            requirement_confirmed: true,
          }),
        );
        const execCycle = asRecord(result.execution_cycle);
        const tgCkpt = asRecord(execCycle.task_graph_checkpoint);
        const ckptId: string =
          typeof tgCkpt.checkpoint_id === "string"
            ? tgCkpt.checkpoint_id
            : "none";
        const resumeEligible: boolean = Boolean(
          tgCkpt.resume_eligible ?? false,
        );
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandCompleted, [
            "task.execute",
            `checkpoint=${ckptId}, resume_eligible=${resumeEligible}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "task.execute",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Hardness status ─────────────────────────────────────────────────
  const hardnessStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.hardnessStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const task = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptHardnessTask),
        placeHolder: i18n.getMessage(MessageKeys.promptHardnessTaskPlaceholder),
        value: "Assess multi-file routing and budget orchestration update",
      });
      if (task === undefined) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("hardness.status", {
            task,
            changed_files: [
              "src/acp/impl/request.rs",
              "tests/acp_runtime_rpc_integration.rs",
            ],
            tool_dependencies: ["search_files", "read_file", "write_file"],
          }),
        );
        const hardness = asRecord(result.hardness);
        const budget = asRecord(hardness.budget);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "hardness.status",
            `level=${String(hardness.level ?? "unknown")}, score=${Number(hardness.score ?? 0).toFixed(1)}, timeout=${Number(budget.timeout_seconds ?? 0)}s, parallelism_cap=${Number(budget.parallelism_cap ?? 1)}, mode=${String(budget.recommended_mode ?? "agent")}, reviews=${Number(budget.required_reviews ?? 1)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "hardness.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Cost status ─────────────────────────────────────────────────────
  const costStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.costStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const task = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptCostTask),
        placeHolder: i18n.getMessage(MessageKeys.promptCostTaskPlaceholder),
        value:
          "Optimize token budget and model cost routing for multi-step task",
      });
      if (task === undefined) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("cost.status", {
            task,
            changed_files: [
              "src/acp/impl/request.rs",
              "vscode-addon/src/extension.ts",
            ],
            tool_dependencies: ["search_files", "read_file", "write_file"],
            max_output_tokens: 1800,
          }),
        );
        const cost = asRecord(result.cost);
        const budget = asRecord(cost.budget);
        const compression = asRecord(cost.compression);
        const routing = asRecord(cost.routing);
        const telemetry = asRecord(cost.telemetry);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "cost.status",
            `class=${String(budget.budget_class ?? "unknown")}, input=${Number(budget.input_tokens_estimate ?? 0)}, output=${Number(budget.output_tokens_budget ?? 0)}, total=${Number(budget.total_tokens_budget ?? 0)}, compress=${Boolean(compression.triggered)}, tier=${String(routing.preferred_model_tier ?? "economy")}, est_cost=${Number(telemetry.estimated_total_cost ?? 0).toFixed(4)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "cost.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Checkpoint create ───────────────────────────────────────────────
  const checkpointCreateRpcCommand = vscode.commands.registerCommand(
    "go-on.checkpointCreate",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const conversationId = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptConversationId),
        placeHolder: i18n.getMessage(
          MessageKeys.promptConversationIdPlaceholder,
        ),
      });
      if (!conversationId) {
        return;
      }
      const message = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptCheckpointMessage),
        placeHolder: i18n.getMessage(
          MessageKeys.promptCheckpointMessagePlaceholder,
        ),
      });
      if (!message) {
        return;
      }
      try {
        const result = await deps.sendRequest(
          "conversation.checkpoint.create",
          {
            conversation_id: conversationId,
            messages: [{ role: "user", content: message }],
          },
        );
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.checkpointCreated, [
            safeStringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "checkpoint.create",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Checkpoint list ─────────────────────────────────────────────────
  const checkpointListRpcCommand = vscode.commands.registerCommand(
    "go-on.checkpointList",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const conversationId = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptConversationId),
        placeHolder: i18n.getMessage(
          MessageKeys.promptConversationIdPlaceholder,
        ),
      });
      if (!conversationId) {
        return;
      }
      try {
        const result = await deps.sendRequest("checkpoint.list", {
          conversation_id: conversationId,
        });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.checkpointsResult, [
            safeStringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "checkpoint.list",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Conversation rollback ───────────────────────────────────────────
  const conversationRollbackRpcCommand = vscode.commands.registerCommand(
    "go-on.conversationRollback",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const checkpointId = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptCheckpointId),
        placeHolder: i18n.getMessage(MessageKeys.promptCheckpointIdPlaceholder),
      });
      if (!checkpointId) {
        return;
      }
      const conversationId = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptConversationId),
        placeHolder: i18n.getMessage(
          MessageKeys.promptConversationIdPlaceholder,
        ),
      });
      if (!conversationId) {
        return;
      }
      try {
        const result = await deps.sendRequest("conversation.rollback", {
          conversation_id: conversationId,
          checkpoint_id: checkpointId,
        });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rolledBack, [safeStringify(result)]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "conversation.rollback",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Action check ────────────────────────────────────────────────────
  const actionCheckRpcCommand = vscode.commands.registerCommand(
    "go-on.actionCheck",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("action.check", { kind: "all" }),
        );
        const report = asRecord(result.report);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "action.check",
            `ok=${Boolean(result.ok)}, checks=${Number(report.total_checks ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "action.check",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Debug panel get ─────────────────────────────────────────────────
  const debugPanelGetRpcCommand = vscode.commands.registerCommand(
    "go-on.debugPanelGet",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(await deps.sendRequest("debug.panel.get", {}));
        const panel = asRecord(result.panel);
        const conversations = asRecord(panel.conversations);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "debug.panel.get",
            `conversations=${Number(conversations.count ?? 0)}, checkpoints=${Number(conversations.checkpoints ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "debug.panel.get",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Primary-secondary summary ───────────────────────────────────────
  const primarySecondarySummaryRpcCommand = vscode.commands.registerCommand(
    "go-on.primarySecondarySummary",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = await deps.sendRequest("primary_secondary.summary", {});
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "primary_secondary.summary",
            safeStringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "primary_secondary.summary",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  return [
    workflowExecuteRpcCommand,
    taskPlanRpcCommand,
    taskExecuteRpcCommand,
    hardnessStatusRpcCommand,
    costStatusRpcCommand,
    checkpointCreateRpcCommand,
    checkpointListRpcCommand,
    conversationRollbackRpcCommand,
    actionCheckRpcCommand,
    debugPanelGetRpcCommand,
    primarySecondarySummaryRpcCommand,
  ];
}
