import * as fs from "fs";
import * as path from "path";
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
 * Register configuration, governance, metrics, quality, and security RPC commands.
 */
export function registerConfigRpcCommands(
  deps: RpcCommandRegistryDeps,
): vscode.Disposable[] {
  // ── Config baseline ─────────────────────────────────────────────────
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
        const capability = String(
          effective.protocol_capability ?? "unknown",
        );
        const dispatch = String(
          effective.request_dispatch_mode ?? "unknown",
        );
        const transport = String(effective.startup_transport ?? "unknown");
        const strictEnabled = effective.production_strict === true;
        const legacyCount = Number(migration.legacy_key_count ?? 0);
        const explicitCount = Number(
          file.runtime_explicit_field_count ?? 0,
        );
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

  // ── Error contract ──────────────────────────────────────────────────
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

  // ── Build reproducibility ──────────────────────────────────────────
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

  // ── Data lifecycle ──────────────────────────────────────────────────
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

  // ── Optimization peak ───────────────────────────────────────────────
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

  // ── Autotune commands ───────────────────────────────────────────────
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
            safeStringify(result),
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
            safeStringify(result),
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
            safeStringify(result),
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

  // ── Selector status ─────────────────────────────────────────────────
  const selectorStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.selectorStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(await deps.sendRequest("selector.status"));
        const selector = asRecord(result.selector);
        const models = asArray(selector.models);
        const topModel = models.length > 0 ? asRecord(models[0]) : {};
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "selector.status",
            `exploration_bias=${Number(selector.exploration_bias ?? 0).toFixed(2)}, tracked_models=${Number(selector.tracked_models ?? 0)}, total_observations=${Number(selector.total_observations ?? 0)}, top_model=${String(topModel.model_id ?? "-")}, top_score=${Number(topModel.ucb_score ?? 0).toFixed(3)}`,
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

  // ── Quality baseline ────────────────────────────────────────────────
  const qualityBaselineRpcCommand = vscode.commands.registerCommand(
    "go-on.qualityBaseline",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        // The three RPCs are independent — fire them in parallel instead of
        // awaiting serially (each request can take up to its own timeout).
        const [healthResult, metricsResult, traceResult] = await Promise.all([
          deps.sendRequest("runtime.health"),
          deps.sendRequest("metrics.get"),
          deps.sendRequest("trace.metrics"),
        ]);

        const metrics = asRecord(metricsResult);
        const trace = asRecord(traceResult);
        const lifecycle = asRecord(asRecord(healthResult).lifecycle);
        const timeouts = asRecord(trace.timeouts);

        const workspaceRoot =
          vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        let scenarioCount = 0;
        if (workspaceRoot) {
          const requestsDir = path.join(workspaceRoot, "requests");
          if (fs.existsSync(requestsDir)) {
            scenarioCount = fs
              .readdirSync(requestsDir)
              .filter((name) =>
                name.toLowerCase().endsWith(".ndjson"),
              ).length;
          }
        }

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "config.baseline",
            `healthy=${Boolean(lifecycle.is_healthy)}, total=${Number(metrics.total_requests ?? 0)}, success=${Number(metrics.successful_requests ?? 0)}, failed=${Number(metrics.failed_requests ?? 0)}, avg_ms=${Number(metrics.avg_request_duration_ms ?? 0).toFixed(1)}, buffered=${Number(trace.buffered_events ?? 0)}, scenarios=${scenarioCount}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}`,
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

  // ── Metrics commands ────────────────────────────────────────────────
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

  // ── Governance commands ─────────────────────────────────────────────
  const governanceStatusRpcCommand = vscode.commands.registerCommand(
    "go-on.governanceStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("governance.status"),
        );
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
        const entryAuthEnabled =
          governanceConfig.entry_auth_enabled === true;
        const entryAuthKeyConfigured =
          governanceConfig.entry_auth_key_configured === true;
        const entryRateLimit = Number(
          governanceConfig.entry_rate_limit_rpm ?? 0,
        );
        const multiUserMode = String(multiUser.mode ?? "single_user");
        const multiUserReady = Boolean(
          multiUserReleaseGate.ready ?? false,
        );
        const multiUserLifecycleReady = Boolean(
          multiUserLifecycle.ready ?? false,
        );
        const multiUserSource = String(
          multiUserInference.source ?? "default",
        );
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
        const result = asRecord(
          await deps.sendRequest("governance.plan.get"),
        );
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
          events.length > 0
            ? asRecord(events[events.length - 1]).action
            : "-";
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

  const governanceAuditVerifyRpcCommand = vscode.commands.registerCommand(
    "go-on.governanceAuditVerify",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("governance.audit.verify", {}),
        );
        const violations = asArray(result.violations).length;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "governance.audit.verify",
            `entries=${result.entry_count ?? 0}, intact=${String(
              result.is_chain_intact,
            )}, violations=${violations}`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "governance.audit.verify",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  // ── Release readiness ───────────────────────────────────────────────
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

  // ── Runtime stability ───────────────────────────────────────────────
  const runtimeStabilityRpcCommand = vscode.commands.registerCommand(
    "go-on.runtimeStability",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      try {
        const result = asRecord(
          await deps.sendRequest("runtime.stability"),
        );
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

  // ── Trace get ───────────────────────────────────────────────────────
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
            safeStringify(result),
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

  // ── Observability alerts ────────────────────────────────────────────
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
          items.length > 0
            ? String(asRecord(items[0]).code ?? "-")
            : "-";
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

  // ── Security baseline ──────────────────────────────────────────────
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

  // ── Circuit breaker commands ────────────────────────────────────────
  const breakerResetRpcCommand = vscode.commands.registerCommand(
    "go-on.breakerReset",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }
      const agent = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptBreakerAgent),
        placeHolder: i18n.getMessage(
          MessageKeys.promptBreakerAgentPlaceholder,
        ),
      });
      if (!agent) {
        return;
      }
      try {
        const result = await deps.sendRequest("breaker.reset", { agent });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "breaker.reset",
            safeStringify(result),
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
        placeHolder: i18n.getMessage(
          MessageKeys.promptBreakerAgentPlaceholder,
        ),
      });
      if (target === undefined) {
        return;
      }
      try {
        const params =
          target.trim().length > 0 ? { agent: target.trim() } : {};
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

  // ── Maintenance GC ──────────────────────────────────────────────────
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

  return [
    configBaselineRpcCommand,
    errorContractRpcCommand,
    buildReproRpcCommand,
    dataLifecycleRpcCommand,
    optimizationPeakRpcCommand,
    autotuneStatusRpcCommand,
    autotuneGetRpcCommand,
    autotuneResetRpcCommand,
    selectorStatusRpcCommand,
    qualityBaselineRpcCommand,
    metricsGetRpcCommand,
    metricsResetRpcCommand,
    traceMetricsRpcCommand,
    governanceStatusRpcCommand,
    governancePlanGetRpcCommand,
    governanceAuditRecentRpcCommand,
    governanceAuditVerifyRpcCommand,
    releaseReadinessRpcCommand,
    runtimeStabilityRpcCommand,
    traceGetRpcCommand,
    observabilityAlertsRpcCommand,
    securityBaselineRpcCommand,
    breakerResetRpcCommand,
    breakerRecoveryRpcCommand,
    maintenanceGcRpcCommand,
  ];
}
