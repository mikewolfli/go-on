import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";
import { isRecord, asRecord } from "./utils";

interface RpcCommandRegistryDeps {
  isRunning: () => boolean;
  sendRequest: (_method: string, _params?: unknown) => Promise<unknown>;
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function ensureRunning(deps: RpcCommandRegistryDeps): Promise<boolean> {
  if (!deps.isRunning()) {
    await vscode.window.showErrorMessage(
      i18n.getMessage(MessageKeys.goOnNotRunningRpc),
    );
    return false;
  }
  return true;
}

export function registerRpcCommands(
  deps: RpcCommandRegistryDeps,
): vscode.Disposable[] {
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
            JSON.stringify(result),
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
            JSON.stringify(result),
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
        const result = await deps.sendRequest("task.execute", {
          task,
          requirement_confirmed: true,
        });
        const execResult = result as Record<string, unknown>;
        const execCycle = execResult.execution_cycle as
          | Record<string, unknown>
          | undefined;
        const tgCkpt = execCycle?.task_graph_checkpoint as
          | Record<string, unknown>
          | undefined;
        const ckptId: string =
          typeof tgCkpt?.checkpoint_id === "string"
            ? tgCkpt.checkpoint_id
            : "none";
        const resumeEligible: boolean = Boolean(
          tgCkpt?.resume_eligible ?? false,
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

  const learningSummaryRpcCommand = vscode.commands.registerCommand(
    "go-on.learningSummary",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = await deps.sendRequest("learning.summary");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "learning.summary",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "learning.summary",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const learningGuardrailRpcCommand = vscode.commands.registerCommand(
    "go-on.learningGuardrail",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("learning.guardrail", { limit: 50 }),
        );
        const guardrail = asRecord(result.guardrail);
        const stats = asRecord(guardrail.stats);
        const warnings = asArray(guardrail.warnings).length;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "learning.guardrail",
            `status=${String(guardrail.status ?? "unknown")}, samples=${Number(stats.records_total ?? 0)}, parseable=${(Number(stats.parseable_ratio ?? 0) * 100).toFixed(1)}%, quality=${(Number(stats.quality_ratio ?? 0) * 100).toFixed(1)}%, high_risk=${Number(stats.high_risk_records ?? 0)}, warnings=${warnings}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "learning.guardrail",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const learningReplayRpcCommand = vscode.commands.registerCommand(
    "go-on.learningReplay",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("learning.replay", { limit: 20 }),
        );
        const replay = asRecord(result.replay);
        const records = asArray(replay.records).length;
        const workflow = Number(replay.workflow_events ?? 0);
        const pua = Number(replay.pua_events ?? 0);
        const hasBus = replay.latest_learning_bus ? "yes" : "no";
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "learning.replay",
            `records=${records}, workflow=${workflow}, pua=${pua}, latest_bus=${hasBus}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "learning.replay",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const knowledgeDistillRpcCommand = vscode.commands.registerCommand(
    "go-on.knowledgeDistill",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("knowledge.distill", {
            limit: 20,
            strategy_limit: 8,
            apply_tombstone: true,
          }),
        );
        const distillation = asRecord(result.distillation);
        const layers = asRecord(distillation.layers);
        const evidence = asRecord(layers.evidence);
        const summary = asRecord(layers.summary);
        const strategy = asRecord(layers.strategy);
        const conflicts = asRecord(layers.conflicts);
        const tombstones = asRecord(layers.tombstones);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "knowledge.distill",
            `evidence=${Number(evidence.records_total ?? 0)}, summary=${Number(summary.sampled_events ?? 0)}, strategy=${Number(strategy.rules_total ?? 0)}, conflicts=${Number(conflicts.count ?? 0)}, tombstones_added=${Number(tombstones.added_count ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "knowledge.distill",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const rlAlignmentEvalRpcCommand = vscode.commands.registerCommand(
    "go-on.rlAlignmentEval",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("rl.alignment.offline_eval", { window: 120 }),
        );
        const offlineEval = asRecord(result.offline_eval);
        const decision = asRecord(offlineEval.decision);
        const comparison = asRecord(offlineEval.comparison);
        const drift = asRecord(offlineEval.drift);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "rl.alignment.offline_eval",
            `samples=${Number(offlineEval.samples_total ?? 0)}, uplift=${Number(comparison.reward_uplift ?? 0).toFixed(4)}, pass=${Boolean(comparison.passes)}, drift=${Number(drift.absolute_diff ?? 0).toFixed(4)}, alert=${Boolean(drift.alert)}, mode=${String(decision.recommended_mode ?? "conservative")}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "rl.alignment.offline_eval",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

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

  const configBaselineRpcCommand = vscode.commands.registerCommand(
    "go-on.configBaseline",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("config.baseline"));
        const baseline = asRecord(result.baseline);
        const effective = asRecord(baseline.effective);
        const migration = asRecord(baseline.migration);
        const file = asRecord(baseline.file);
        const status = String(baseline.status ?? "unknown");
        const configuredMode = String(
          effective.configured_mode ?? effective.protocol_mode ?? "adaptive",
        );
        const capability = String(effective.protocol_capability ?? "unknown");
        const dispatch = String(effective.request_dispatch_mode ?? "unknown");
        const transport = String(effective.startup_transport ?? "unknown");
        const strictEnabled = effective.production_strict === true;
        const legacyCount = Number(migration.legacy_key_count ?? 0);
        const explicitCount = Number(file.runtime_explicit_field_count ?? 0);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "config.baseline",
            `status=${status}, mode=${configuredMode}, capability=${capability}, dispatch=${dispatch}, transport=${transport}, strict=${strictEnabled ? "on" : "off"}, runtime_explicit=${explicitCount}, legacy_keys=${legacyCount}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "config.baseline",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const errorContractRpcCommand = vscode.commands.registerCommand(
    "go-on.errorContract",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("error.contract"));
        const contract = asRecord(result.contract);
        const version = String(contract.version ?? "unknown");
        const kinds = asArray(contract.kinds);
        const retryableKinds = kinds.filter((item) => {
          const entry = asRecord(item);
          const retry = asRecord(entry.retry);
          return retry.retryable === true;
        }).length;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "error.contract",
            `version=${version}, kinds=${kinds.length}, retryable_kinds=${retryableKinds}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "error.contract",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const buildReproRpcCommand = vscode.commands.registerCommand(
    "go-on.buildRepro",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("build.repro"));
        const build = asRecord(result.build);
        const repro = asRecord(build.reproducibility);
        const buildMeta = asRecord(build.build);
        const releaseManifest = asRecord(build.release_manifest);
        const items = asArray(releaseManifest.items);
        const requiredPresent = Number(repro.required_present ?? 0);
        const requiredTotal = Number(repro.required_total ?? 0);
        const status = String(build.status ?? "unknown");
        const commit = String(buildMeta.git_commit_short ?? "-");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "build.repro",
            `status=${status}, required=${requiredPresent}/${requiredTotal}, commit=${commit}, release_items=${items.length}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "build.repro",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const dataLifecycleRpcCommand = vscode.commands.registerCommand(
    "go-on.dataLifecycle",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("data.lifecycle", { execute_gc: false }),
        );
        const lifecycle = asRecord(result.lifecycle);
        const storage = asRecord(lifecycle.storage);
        const waterline = asRecord(storage.waterline);
        const status = String(waterline.status ?? "unknown");
        const totalBytes = Number(storage.total_bytes ?? 0);
        const targetCount = asArray(storage.targets).length;
        const alerts = asArray(waterline.alerts).length;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "data.lifecycle",
            `status=${status}, total_bytes=${totalBytes}, targets=${targetCount}, alerts=${alerts}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "data.lifecycle",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const optimizationPeakRpcCommand = vscode.commands.registerCommand(
    "go-on.optimizationPeak",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("optimization.peak", {
            task: "BLUE15 one-shot optimization peak",
            freeze_mode: "strict",
          }),
        );
        const peak = asRecord(result.peak);
        const gates = asArray(peak.gates);
        const passed = gates.filter(
          (item) => asRecord(item).passed === true,
        ).length;
        const overallPass = peak.overall_pass === true;
        const status = String(peak.status ?? "unknown");
        const version = String(peak.version ?? "-");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "optimization.peak",
            `status=${status}, overall_pass=${overallPass}, gates=${passed}/${gates.length}, version=${version}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "optimization.peak",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const autotuneStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.autotuneStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = await deps.sendRequest("autotune.status");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "autotune.status",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "autotune.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const selectorStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.selectorStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("selector.status"));
        const mode = String(result.mode ?? "unknown");
        const selector = asRecord(result.selector);
        const models = asArray(selector.models);
        const topModel = models.length > 0 ? asRecord(models[0]) : {};
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "selector.status",
            `mode=${mode}, exploration_bias=${Number(selector.exploration_bias ?? 0).toFixed(2)}, tracked_models=${Number(selector.tracked_models ?? 0)}, total_observations=${Number(selector.total_observations ?? 0)}, top_model=${String(topModel.model_id ?? "-")}, top_score=${Number(topModel.ucb_score ?? 0).toFixed(3)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "selector.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const governanceStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.governanceStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("governance.status"));
        const governance = asRecord(result.governance);
        const governanceConfig = asRecord(governance.config);
        const rules = asRecord(governance.rules);
        const artifactContract = asRecord(governance.artifact_contract);
        const dualTrack = asRecord(governance.dual_track_consistency);
        const multiUser = asRecord(governance.multi_user_server);
        const multiUserLifecycle = asRecord(multiUser.lifecycle);
        const multiUserInference = asRecord(multiUser.inference);
        const multiUserReleaseGate = asRecord(multiUser.release_gate);
        const strictEnabled = governanceConfig.production_strict === true;
        const strictViolations = Number(
          governanceConfig.strict_violation_count ?? 0,
        );
        const entryAuthEnabled = governanceConfig.entry_auth_enabled === true;
        const entryAuthKeyConfigured =
          governanceConfig.entry_auth_key_configured === true;
        const entryRateLimit = Number(
          governanceConfig.entry_rate_limit_rpm ?? 0,
        );
        const multiUserMode = String(multiUser.mode ?? "single_user");
        const multiUserReady = Boolean(multiUserReleaseGate.ready ?? false);
        const multiUserLifecycleReady = Boolean(
          multiUserLifecycle.ready ?? false,
        );
        const multiUserSource = String(multiUserInference.source ?? "default");
        const dualTrackReady = Boolean(dualTrack.ready ?? false);
        const governanceSchema = String(
          governance.schema_version ??
            artifactContract.schema_version ??
            "unknown",
        );
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "governance.status",
            `governance=${governance.status ?? "unknown"}, schema=${governanceSchema}, strict=${strictEnabled ? "on" : "off"}, strict_violations=${strictViolations}, entry_auth=${entryAuthEnabled ? "on" : "off"}, entry_key=${entryAuthKeyConfigured ? "set" : "missing"}, entry_rpm=${entryRateLimit}, multi_user_mode=${multiUserMode}, multi_user_ready=${multiUserReady}, multi_user_lifecycle_ready=${multiUserLifecycleReady}, dual_track_ready=${dualTrackReady}, multi_user_source=${multiUserSource}, rules=${rules.version ?? "-"}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "governance.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const governancePlanGetRpcCommand = vscode.commands.registerCommand(
    "go-on.governancePlanGet",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("governance.plan.get"));
        const plan = asRecord(result.plan);
        const escalationLevel = String(plan.escalation_level ?? "L1");
        const redLines = asArray(plan.red_lines).length;
        const stageReq = asArray(plan.stage_requirements).length;
        const safeguards = asArray(plan.mandatory_safeguards).length;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "governance.plan.get",
            `escalation=${escalationLevel}, red_lines=${redLines}, stage_requirements=${stageReq}, safeguards=${safeguards}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "governance.plan.get",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const governanceAuditRecentRpcCommand = vscode.commands.registerCommand(
    "go-on.governanceAuditRecent",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      const limitText = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptAuditLimit),
        placeHolder: "20",
        value: "20",
      });

      if (limitText === undefined) {
        return;
      }

      const limit = Number.parseInt(limitText, 10);
      const safeLimit =
        Number.isFinite(limit) && limit > 0 ? Math.min(limit, 200) : 20;

      try {
        const result = asRecord(
          await deps.sendRequest("governance.audit.recent", {
            limit: safeLimit,
          }),
        );
        const audit = asRecord(result.audit);
        const events = asArray(audit.events);
        const latestRaw =
          events.length > 0 ? asRecord(events[events.length - 1]).action : "-";
        const latestAction = String(latestRaw ?? "-");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "governance.audit.recent",
            `events=${events.length}, latest_action=${latestAction}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "governance.audit.recent",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const skillListImportedRpcCommand = vscode.commands.registerCommand(
    "go-on.skillListImported",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(await deps.sendRequest("skill.list", {}));
        const skills = asArray(result.skills);
        const enabled = Number(
          result.enabled ??
            skills.filter((item) => asRecord(item).enabled === true).length,
        );
        const total = Number(result.total ?? skills.length);
        const disabled = Number(
          result.disabled ?? Math.max(0, total - enabled),
        );
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "skill.list",
            `total=${total}, enabled=${enabled}, disabled=${disabled}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.list",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const skillImportLocalRpcCommand = vscode.commands.registerCommand(
    "go-on.skillImportLocal",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const manifestPath = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptSkillManifestPath),
        placeHolder: i18n.getMessage(
          MessageKeys.promptSkillManifestPathPlaceholder,
        ),
      });
      if (!manifestPath) {
        return;
      }
      const sha256 = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptSkillSha256),
        placeHolder: i18n.getMessage(MessageKeys.promptSkillSha256Placeholder),
      });
      try {
        const result = asRecord(
          await deps.sendRequest("skill.import", {
            source: {
              kind: "local",
              path: manifestPath,
              sha256: sha256?.trim() || undefined,
            },
          }),
        );
        const skill = asRecord(result.skill);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "skill.import",
            `name=${String(skill.name ?? "-")}, version=${String(skill.version ?? "-")}, enabled=${Boolean(skill.enabled)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.import",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const skillToggleRpcCommand = vscode.commands.registerCommand(
    "go-on.skillToggle",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      const name = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptSkillName),
        placeHolder: i18n.getMessage(MessageKeys.promptSkillNamePlaceholder),
      });
      if (!name) {
        return;
      }

      const action = await vscode.window.showQuickPick(
        ["enable", "disable", "remove"],
        {
          placeHolder: i18n.getMessage(MessageKeys.promptSkillAction),
        },
      );
      if (!action) {
        return;
      }

      try {
        const method =
          action === "enable"
            ? "skill.enable"
            : action === "disable"
              ? "skill.disable"
              : "skill.remove";
        const result = asRecord(await deps.sendRequest(method, { name }));
        const removed = Boolean(result.removed);
        const skill = asRecord(result.skill);
        if (action === "remove") {
          vscode.window.showInformationMessage(
            i18n.getMessage(MessageKeys.rpcCommandResult, [
              "skill.remove",
              `name=${name}, removed=${removed}`,
            ]),
          );
          return;
        }
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            method,
            `name=${String(skill.name ?? name)}, enabled=${Boolean(skill.enabled)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const autotuneGetRpcCommand = vscode.commands.registerCommand(
    "go-on.autotuneGet",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = await deps.sendRequest("autotune.get");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "autotune.get",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "autotune.get",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const autotuneResetRpcCommand = vscode.commands.registerCommand(
    "go-on.autotuneReset",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const confirmAutotune = await vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.autotuneResetConfirm),
        i18n.getMessage(MessageKeys.resetButton),
        i18n.getMessage(MessageKeys.cancel),
      );
      if (confirmAutotune !== i18n.getMessage(MessageKeys.resetButton)) {
        return;
      }
      try {
        const result = await deps.sendRequest("autotune.reset", {});
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "autotune.reset",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "autotune.reset",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const metricsGetRpcCommand = vscode.commands.registerCommand(
    "go-on.metricsGet",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const metrics = asRecord(await deps.sendRequest("metrics.get"));
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "metrics",
            `chat=${Number(metrics.chat_requests_total ?? 0)}, failed=${Number(metrics.failed_requests ?? 0)}, agent_timeout=${Number(metrics.agent_timeout_failures_total ?? 0)}, review_timeout=${Number(metrics.review_gate_timeout_total ?? 0)}, probe_timeout=${Number(metrics.runtime_probe_timeout_total ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "metrics.get",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const metricsResetRpcCommand = vscode.commands.registerCommand(
    "go-on.metricsReset",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const confirmMetrics = await vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.metricsResetConfirm),
        i18n.getMessage(MessageKeys.resetButton),
        i18n.getMessage(MessageKeys.cancel),
      );
      if (confirmMetrics !== i18n.getMessage(MessageKeys.resetButton)) {
        return;
      }
      try {
        await deps.sendRequest("metrics.reset");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.metricsResetCompleted),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "metrics.reset",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const traceMetricsRpcCommand = vscode.commands.registerCommand(
    "go-on.traceMetrics",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const trace = asRecord(await deps.sendRequest("trace.metrics"));
        const timeouts = asRecord(trace.timeouts);
        const topN = asArray(trace.slow_requests_top_n).length;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "trace.metrics",
            `buffered=${Number(trace.buffered_events ?? 0)}, slow_top_n=${topN}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "trace.metrics",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const qualityBaselineRpcCommand = vscode.commands.registerCommand(
    "go-on.qualityBaseline",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const healthResult = asRecord(await deps.sendRequest("runtime.health"));
        const metrics = asRecord(await deps.sendRequest("metrics.get"));
        const trace = asRecord(await deps.sendRequest("trace.metrics"));

        const lifecycle = asRecord(healthResult.lifecycle);
        const timeouts = asRecord(trace.timeouts);

        const workspaceRoot =
          vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        let scenarioCount = 0;
        if (workspaceRoot) {
          const requestsDir = path.join(workspaceRoot, "requests");
          if (fs.existsSync(requestsDir)) {
            scenarioCount = fs
              .readdirSync(requestsDir)
              .filter((name) => name.toLowerCase().endsWith(".ndjson")).length;
          }
        }

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "quality.baseline",
            `healthy=${Boolean(lifecycle.is_healthy)}, total=${Number(metrics.total_requests ?? 0)}, success=${Number(metrics.successful_requests ?? 0)}, failed=${Number(metrics.failed_requests ?? 0)}, avg_ms=${Number(metrics.avg_request_duration_ms ?? 0).toFixed(1)}, buffered=${Number(trace.buffered_events ?? 0)}, scenarios=${scenarioCount}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "quality.baseline",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const runtimeStabilityRpcCommand = vscode.commands.registerCommand(
    "go-on.runtimeStability",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(await deps.sendRequest("runtime.stability"));
        const stability = asRecord(result.stability);
        const checks = asArray(stability.checks);
        const summary = asRecord(stability.summary);
        const checkSummary = checks
          .map((check) => {
            const checkEntry = asRecord(check);
            return `${String(checkEntry.name ?? "-")}=${String(checkEntry.status ?? "-")}`;
          })
          .join(", ");

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "runtime.stability",
            `score=${Number(stability.score ?? 0)}, level=${stability.level ?? "unknown"}, safe_restart=${Boolean(stability.safe_restart_ready)}, health_errors=${Number(summary.health_errors ?? 0)}, health_warnings=${Number(summary.health_warnings ?? 0)}, config_warnings=${Number(summary.config_warnings ?? 0)}, strict_violations=${Number(summary.strict_violations ?? 0)}, checks=[${checkSummary}]`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "runtime.stability",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const runtimeSelfModelRpcCommand = vscode.commands.registerCommand(
    "go-on.runtimeSelfModel",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("runtime.self_model", { window: 120 }),
        );
        const selfModel = asRecord(result.self_model);
        const health = asRecord(selfModel.health);
        const readiness = asRecord(health.readiness);
        const stability = asRecord(selfModel.stability);
        const drift = asRecord(selfModel.drift);
        const decision = asRecord(selfModel.decision);
        const recommendations = asArray(selfModel.recommendations);

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "runtime.self_model",
            `readiness=${String(readiness.status ?? "unknown")}, stability=${String(stability.level ?? "unknown")}, safe_restart=${Boolean(stability.safe_restart_ready)}, mode=${String(decision.recommended_mode ?? "unknown")}, drift_alert=${Boolean(drift.alert)}, drift_diff=${Number(drift.absolute_diff ?? 0).toFixed(4)}, recommendations=${recommendations.length}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "runtime.self_model",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const providerStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.providerStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(await deps.sendRequest("provider.status", {}));
        const providerStatus = asRecord(result.provider_status);
        const summary = asRecord(providerStatus.summary);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "provider.status",
            `status=${String(providerStatus.status ?? "unknown")}, ready=${Number(summary.ready ?? 0)}, degraded=${Number(summary.degraded ?? 0)}, configured=${Number(summary.configured ?? 0)}, coverage=${Number(summary.coverage_percent ?? 0)}%`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "provider.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const releaseReadinessRpcCommand = vscode.commands.registerCommand(
    "go-on.releaseReadiness",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("release.readiness", {}),
        );
        const readiness = asRecord(result.readiness);
        const summary = asRecord(readiness.summary);
        const artifactContract = asRecord(readiness.artifact_contract);
        const dualTrack = asRecord(readiness.dual_track_consistency);
        const multiUser = asRecord(readiness.multi_user_server);
        const lifecycle = asRecord(multiUser.lifecycle);
        const inference = asRecord(multiUser.inference);
        const blockedGateNames = asArray(readiness.blocked_gate_names)
          .map((item) => String(item))
          .join("|");
        const readinessSchema = String(
          readiness.schema_version ??
            artifactContract.schema_version ??
            readiness.version ??
            "unknown",
        );
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "release.readiness",
            `status=${String(readiness.status ?? "unknown")}, schema=${readinessSchema}, overall=${Boolean(readiness.overall_pass)}, blocked=${Number(readiness.blocked_gate_count ?? 0)}, blocked_names=${blockedGateNames || "-"}, open_breakers=${Number(summary.open_breakers ?? 0)}, degraded_services=${Number(summary.degraded_services ?? 0)}, multi_user_mode=${String(multiUser.mode ?? "single_user")}, multi_user_ready=${Boolean(multiUser.release_gate_ready ?? false)}, multi_user_lifecycle_ready=${Boolean(lifecycle.ready ?? summary.multi_user_lifecycle_ready ?? false)}, dual_track_ready=${Boolean(dualTrack.ready ?? summary.dual_track_consistency_ready ?? false)}, multi_user_source=${String(inference.source ?? summary.multi_user_inference_source ?? "default")}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "release.readiness",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const harnessStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.harnessStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("harness.status", { seed: 20260415 }),
        );
        const harness = asRecord(result.harness);
        const suites = asRecord(harness.suites);
        const smoke = asRecord(suites.smoke);
        const regression = asRecord(suites.regression);
        const adversarial = asRecord(suites.adversarial);
        const longChain = asRecord(suites.long_chain);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "harness.status",
            `total=${Number(harness.scenario_total ?? 0)}, smoke=${Number(smoke.count ?? 0)}, regression=${Number(regression.count ?? 0)}, adversarial=${Number(adversarial.count ?? 0)}, long_chain=${Number(longChain.count ?? 0)}, seed=${Number(harness.fixed_seed ?? 0)}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "harness.status",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const traceGetRpcCommand = vscode.commands.registerCommand(
    "go-on.traceGet",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = await deps.sendRequest("trace.get", {});
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "trace.get",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "trace.get",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const observabilityAlertsRpcCommand = vscode.commands.registerCommand(
    "go-on.observabilityAlerts",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("observability.alerts", { limit: 20 }),
        );
        const alerts = asRecord(result.alerts);
        const items = asArray(alerts.items);
        const topCode =
          items.length > 0 ? String(asRecord(items[0]).code ?? "-") : "-";
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "observability.alerts",
            `critical=${Number(alerts.critical ?? 0)}, warn=${Number(alerts.warn ?? 0)}, info=${Number(alerts.info ?? 0)}, top=${topCode}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "observability.alerts",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const securityBaselineRpcCommand = vscode.commands.registerCommand(
    "go-on.securityBaseline",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("security.baseline", {}),
        );
        const baseline = asRecord(result.baseline);
        const productionStrict = asRecord(baseline.production_strict);
        const level = String(baseline.level ?? "unknown");
        const ingress = String(baseline.ingress_status ?? "unknown");
        const riskCount = Number(baseline.risk_count ?? 0);
        const strict = Boolean(productionStrict.enabled ?? false);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "security.baseline",
            `level=${level}, ingress=${ingress}, strict=${strict}, risks=${riskCount}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "security.baseline",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const breakerResetRpcCommand = vscode.commands.registerCommand(
    "go-on.breakerReset",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const agent = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptBreakerAgent),
        placeHolder: i18n.getMessage(MessageKeys.promptBreakerAgentPlaceholder),
      });
      if (!agent) {
        return;
      }
      try {
        const result = await deps.sendRequest("breaker.reset", { agent });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "breaker.reset",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "breaker.reset",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const breakerRecoveryRpcCommand = vscode.commands.registerCommand(
    "go-on.breakerRecovery",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      const target = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptRecoveryAgent),
        placeHolder: i18n.getMessage(MessageKeys.promptBreakerAgentPlaceholder),
      });
      if (target === undefined) {
        return;
      }

      try {
        const params = target.trim().length > 0 ? { agent: target.trim() } : {};
        const result = asRecord(
          await deps.sendRequest("breaker.recovery", params),
        );
        const recoveredCount = Number(result.recovered_count ?? 0);
        const remaining = Number(result.remaining_degraded_count ?? 0);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "breaker.recovery",
            `recovered=${recoveredCount}, remaining_degraded=${remaining}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "breaker.recovery",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const maintenanceGcRpcCommand = vscode.commands.registerCommand(
    "go-on.maintenanceGc",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        await deps.sendRequest("maintenance.gc");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.maintenanceGcCompleted),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "maintenance.gc",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

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
            JSON.stringify(result),
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
            JSON.stringify(result),
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
          i18n.getMessage(MessageKeys.rolledBack, [JSON.stringify(result)]),
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
            JSON.stringify(result),
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

  return [
    workflowExecuteRpcCommand,
    taskPlanRpcCommand,
    taskExecuteRpcCommand,
    learningSummaryRpcCommand,
    learningGuardrailRpcCommand,
    learningReplayRpcCommand,
    knowledgeDistillRpcCommand,
    rlAlignmentEvalRpcCommand,
    hardnessStatusRpcCommand,
    costStatusRpcCommand,
    configBaselineRpcCommand,
    errorContractRpcCommand,
    buildReproRpcCommand,
    dataLifecycleRpcCommand,
    optimizationPeakRpcCommand,
    autotuneStatusRpcCommand,
    selectorStatusRpcCommand,
    governanceStatusRpcCommand,
    governancePlanGetRpcCommand,
    governanceAuditRecentRpcCommand,
    skillListImportedRpcCommand,
    skillImportLocalRpcCommand,
    skillToggleRpcCommand,
    autotuneGetRpcCommand,
    autotuneResetRpcCommand,
    metricsGetRpcCommand,
    metricsResetRpcCommand,
    traceMetricsRpcCommand,
    qualityBaselineRpcCommand,
    runtimeSelfModelRpcCommand,
    providerStatusRpcCommand,
    releaseReadinessRpcCommand,
    runtimeStabilityRpcCommand,
    harnessStatusRpcCommand,
    traceGetRpcCommand,
    observabilityAlertsRpcCommand,
    securityBaselineRpcCommand,
    breakerResetRpcCommand,
    breakerRecoveryRpcCommand,
    maintenanceGcRpcCommand,
    checkpointCreateRpcCommand,
    checkpointListRpcCommand,
    conversationRollbackRpcCommand,
    primarySecondarySummaryRpcCommand,
    debugPanelGetRpcCommand,
    actionCheckRpcCommand,
  ];
}
