import * as vscode from "vscode";
import { i18n, MessageKeys } from "../i18n";
import {
  asArray,
  asRecord,
  getErrorMessage,
  safeStringify,
  ensureRunning,
  RpcCommandRegistryDeps,
} from "./rpcShared";

/**
 * Register agent-related RPC commands: skills, learning, knowledge, RL alignment,
 * provider status, and self-model introspection.
 */
export function registerAgentRpcCommands(
  deps: RpcCommandRegistryDeps,
): vscode.Disposable[] {
  // ── Skill commands ───────────────────────────────────────────────────
  const skillListImportedRpcCommand = vscode.commands.registerCommand(
    "go-on.skillListImported",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(await deps.sendRequest("skill.list_imported"));
        const skills = asArray(result.skills);
        const enabled = skills.filter(
          (s: unknown) => (s as Record<string, unknown>).enabled === true,
        ).length;
        const total = skills.length;
        const disabled = total - enabled;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "skill.list_imported",
            `${enabled} enabled, ${disabled} disabled (${total} total)`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.list_imported",
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
      if (!manifestPath) return;
      const sha256 = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptSkillSha256),
        placeHolder: i18n.getMessage(MessageKeys.promptSkillSha256Placeholder),
      });
      try {
        const result = asRecord(
          await deps.sendRequest("skill.import_local", {
            source: {
              kind: "local",
              path: manifestPath,
              sha256: sha256 || undefined,
            },
          }),
        );
        const skill = result.skill as Record<string, unknown> | undefined;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "skill.import_local",
            skill?.name
              ? `imported "${String(skill.name)}"`
              : "import succeeded",
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.import_local",
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
      if (!name) return;
      const action = await vscode.window.showQuickPick(
        ["enable", "disable", "remove"],
        { placeHolder: i18n.getMessage(MessageKeys.promptSkillAction) },
      );
      if (!action) return;
      const method =
        action === "enable"
          ? "skill.enable"
          : action === "disable"
            ? "skill.disable"
            : "skill.remove";
      try {
        const result = asRecord(await deps.sendRequest(method, { name }));
        const removed = result.removed as boolean | undefined;
        const skill = result.skill as Record<string, unknown> | undefined;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            method,
            removed
              ? "removed"
              : skill?.name
                ? `"${String(skill.name)}" ${action}d`
                : `${action}d`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.toggle",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Learning commands ────────────────────────────────────────────────
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
            safeStringify(result),
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
        const workflow = Number(replay.workflow_records ?? 0);
        const pua = Number(replay.pua_records ?? 0);
        const hasBus = replay.learning_bus ? "yes" : "no";
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
          await deps.sendRequest("rl.alignment.offline_eval", {
            window: 120,
          }),
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

  // ── Runtime self-model ──────────────────────────────────────────────
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

  // ── Provider status ─────────────────────────────────────────────────
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

  // ── Harness status ──────────────────────────────────────────────────
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

  return [
    skillListImportedRpcCommand,
    skillImportLocalRpcCommand,
    skillToggleRpcCommand,
    learningSummaryRpcCommand,
    learningGuardrailRpcCommand,
    learningReplayRpcCommand,
    knowledgeDistillRpcCommand,
    rlAlignmentEvalRpcCommand,
    runtimeSelfModelRpcCommand,
    providerStatusRpcCommand,
    harnessStatusRpcCommand,
  ];
}
