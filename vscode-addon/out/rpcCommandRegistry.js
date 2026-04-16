"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.registerRpcCommands = void 0;
const fs = require("fs");
const path = require("path");
const vscode = require("vscode");
function isRecord(value) {
    return typeof value === 'object' && value !== null;
}
function asRecord(value) {
    return isRecord(value) ? value : {};
}
function asArray(value) {
    return Array.isArray(value) ? value : [];
}
function getErrorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}
function ensureRunning(deps) {
    if (!deps.isRunning()) {
        vscode.window.showErrorMessage('Go-On is not running. Start it first.');
        return false;
    }
    return true;
}
function registerRpcCommands(deps) {
    const workflowExecuteRpcCommand = vscode.commands.registerCommand('go-on.workflowExecute', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const objective = await vscode.window.showInputBox({
            prompt: 'Workflow objective',
            placeHolder: 'Describe the task objective for workflow.execute'
        });
        if (!objective) {
            return;
        }
        try {
            const result = await deps.sendRequest('workflow.execute', { task: objective });
            vscode.window.showInformationMessage(`workflow.execute completed: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`workflow.execute failed: ${getErrorMessage(error)}`);
        }
    });
    const taskPlanRpcCommand = vscode.commands.registerCommand('go-on.taskPlan', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task to plan',
            placeHolder: 'Describe the task for task.plan'
        });
        if (!task) {
            return;
        }
        try {
            const result = await deps.sendRequest('task.plan', { task });
            vscode.window.showInformationMessage(`task.plan completed: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`task.plan failed: ${getErrorMessage(error)}`);
        }
    });
    const taskExecuteRpcCommand = vscode.commands.registerCommand('go-on.taskExecute', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task to execute',
            placeHolder: 'Describe the task for task.execute'
        });
        if (!task) {
            return;
        }
        try {
            const result = await deps.sendRequest('task.execute', {
                task,
                requirement_confirmed: true,
            });
            vscode.window.showInformationMessage(`task.execute completed: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`task.execute failed: ${getErrorMessage(error)}`);
        }
    });
    const learningSummaryRpcCommand = vscode.commands.registerCommand('go-on.learningSummary', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = await deps.sendRequest('learning.summary');
            vscode.window.showInformationMessage(`learning.summary: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`learning.summary failed: ${getErrorMessage(error)}`);
        }
    });
    const learningGuardrailRpcCommand = vscode.commands.registerCommand('go-on.learningGuardrail', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('learning.guardrail', { limit: 50 }));
            const guardrail = asRecord(result.guardrail);
            const stats = asRecord(guardrail.stats);
            const warnings = asArray(guardrail.warnings).length;
            vscode.window.showInformationMessage(`learning.guardrail: status=${String(guardrail.status ?? 'unknown')}, samples=${Number(stats.records_total ?? 0)}, parseable=${(Number(stats.parseable_ratio ?? 0) * 100).toFixed(1)}%, quality=${(Number(stats.quality_ratio ?? 0) * 100).toFixed(1)}%, high_risk=${Number(stats.high_risk_records ?? 0)}, warnings=${warnings}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`learning.guardrail failed: ${getErrorMessage(error)}`);
        }
    });
    const learningReplayRpcCommand = vscode.commands.registerCommand('go-on.learningReplay', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('learning.replay', { limit: 20 }));
            const replay = asRecord(result.replay);
            const records = asArray(replay.records).length;
            const workflow = Number(replay.workflow_events ?? 0);
            const pua = Number(replay.pua_events ?? 0);
            const hasBus = replay.latest_learning_bus ? 'yes' : 'no';
            vscode.window.showInformationMessage(`learning.replay: records=${records}, workflow=${workflow}, pua=${pua}, latest_bus=${hasBus}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`learning.replay failed: ${getErrorMessage(error)}`);
        }
    });
    const knowledgeDistillRpcCommand = vscode.commands.registerCommand('go-on.knowledgeDistill', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('knowledge.distill', {
                limit: 20,
                strategy_limit: 8,
                apply_tombstone: true,
            }));
            const distillation = asRecord(result.distillation);
            const layers = asRecord(distillation.layers);
            const evidence = asRecord(layers.evidence);
            const summary = asRecord(layers.summary);
            const strategy = asRecord(layers.strategy);
            const conflicts = asRecord(layers.conflicts);
            const tombstones = asRecord(layers.tombstones);
            vscode.window.showInformationMessage(`knowledge.distill: evidence=${Number(evidence.records_total ?? 0)}, summary=${Number(summary.sampled_events ?? 0)}, strategy=${Number(strategy.rules_total ?? 0)}, conflicts=${Number(conflicts.count ?? 0)}, tombstones_added=${Number(tombstones.added_count ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`knowledge.distill failed: ${getErrorMessage(error)}`);
        }
    });
    const rlAlignmentEvalRpcCommand = vscode.commands.registerCommand('go-on.rlAlignmentEval', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('rl.alignment.offline_eval', { window: 120 }));
            const offlineEval = asRecord(result.offline_eval);
            const decision = asRecord(offlineEval.decision);
            const comparison = asRecord(offlineEval.comparison);
            const drift = asRecord(offlineEval.drift);
            vscode.window.showInformationMessage(`rl.alignment.offline_eval: samples=${Number(offlineEval.samples_total ?? 0)}, uplift=${Number(comparison.reward_uplift ?? 0).toFixed(4)}, pass=${Boolean(comparison.passes)}, drift=${Number(drift.absolute_diff ?? 0).toFixed(4)}, alert=${Boolean(drift.alert)}, mode=${String(decision.recommended_mode ?? 'conservative')}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`rl.alignment.offline_eval failed: ${getErrorMessage(error)}`);
        }
    });
    const hardnessStatusRpcCommand = vscode.commands.registerCommand('go-on.hardnessStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task text for hardness.status',
            placeHolder: 'Describe the task to evaluate routing hardness',
            value: 'Assess multi-file routing and budget orchestration update'
        });
        if (task === undefined) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('hardness.status', {
                task,
                changed_files: ['src/acp/impl/request.rs', 'tests/acp_runtime_rpc_integration.rs'],
                tool_dependencies: ['search_files', 'read_file', 'write_file']
            }));
            const hardness = asRecord(result.hardness);
            const budget = asRecord(hardness.budget);
            vscode.window.showInformationMessage(`hardness.status: level=${String(hardness.level ?? 'unknown')}, score=${Number(hardness.score ?? 0).toFixed(1)}, timeout=${Number(budget.timeout_seconds ?? 0)}s, parallelism_cap=${Number(budget.parallelism_cap ?? 1)}, mode=${String(budget.recommended_mode ?? 'agent')}, reviews=${Number(budget.required_reviews ?? 1)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`hardness.status failed: ${getErrorMessage(error)}`);
        }
    });
    const costStatusRpcCommand = vscode.commands.registerCommand('go-on.costStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task text for cost.status',
            placeHolder: 'Describe the task to evaluate token/cost governance',
            value: 'Optimize token budget and model cost routing for multi-step task'
        });
        if (task === undefined) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('cost.status', {
                task,
                changed_files: ['src/acp/impl/request.rs', 'vscode-addon/src/extension.ts'],
                tool_dependencies: ['search_files', 'read_file', 'write_file'],
                max_output_tokens: 1800
            }));
            const cost = asRecord(result.cost);
            const budget = asRecord(cost.budget);
            const compression = asRecord(cost.compression);
            const routing = asRecord(cost.routing);
            const telemetry = asRecord(cost.telemetry);
            vscode.window.showInformationMessage(`cost.status: class=${String(budget.budget_class ?? 'unknown')}, input=${Number(budget.input_tokens_estimate ?? 0)}, output=${Number(budget.output_tokens_budget ?? 0)}, total=${Number(budget.total_tokens_budget ?? 0)}, compress=${Boolean(compression.triggered)}, tier=${String(routing.preferred_model_tier ?? 'economy')}, est_cost=${Number(telemetry.estimated_total_cost ?? 0).toFixed(4)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`cost.status failed: ${getErrorMessage(error)}`);
        }
    });
    const configBaselineRpcCommand = vscode.commands.registerCommand('go-on.configBaseline', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('config.baseline'));
            const baseline = asRecord(result.baseline);
            const effective = asRecord(baseline.effective);
            const migration = asRecord(baseline.migration);
            const file = asRecord(baseline.file);
            const status = String(baseline.status ?? 'unknown');
            const protocolMode = String(effective.protocol_mode ?? 'auto');
            const strictEnabled = effective.production_strict === true;
            const legacyCount = Number(migration.legacy_key_count ?? 0);
            const explicitCount = Number(file.runtime_explicit_field_count ?? 0);
            vscode.window.showInformationMessage(`config.baseline: status=${status}, protocol=${protocolMode}, strict=${strictEnabled ? 'on' : 'off'}, runtime_explicit=${explicitCount}, legacy_keys=${legacyCount}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`config.baseline failed: ${getErrorMessage(error)}`);
        }
    });
    const errorContractRpcCommand = vscode.commands.registerCommand('go-on.errorContract', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('error.contract'));
            const contract = asRecord(result.contract);
            const version = String(contract.version ?? 'unknown');
            const kinds = asArray(contract.kinds);
            const retryableKinds = kinds.filter((item) => {
                const entry = asRecord(item);
                const retry = asRecord(entry.retry);
                return retry.retryable === true;
            }).length;
            vscode.window.showInformationMessage(`error.contract: version=${version}, kinds=${kinds.length}, retryable_kinds=${retryableKinds}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`error.contract failed: ${getErrorMessage(error)}`);
        }
    });
    const buildReproRpcCommand = vscode.commands.registerCommand('go-on.buildRepro', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('build.repro'));
            const build = asRecord(result.build);
            const repro = asRecord(build.reproducibility);
            const buildMeta = asRecord(build.build);
            const releaseManifest = asRecord(build.release_manifest);
            const items = asArray(releaseManifest.items);
            const requiredPresent = Number(repro.required_present ?? 0);
            const requiredTotal = Number(repro.required_total ?? 0);
            const status = String(build.status ?? 'unknown');
            const commit = String(buildMeta.git_commit_short ?? '-');
            vscode.window.showInformationMessage(`build.repro: status=${status}, required=${requiredPresent}/${requiredTotal}, commit=${commit}, release_items=${items.length}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`build.repro failed: ${getErrorMessage(error)}`);
        }
    });
    const dataLifecycleRpcCommand = vscode.commands.registerCommand('go-on.dataLifecycle', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('data.lifecycle', { execute_gc: false }));
            const lifecycle = asRecord(result.lifecycle);
            const storage = asRecord(lifecycle.storage);
            const waterline = asRecord(storage.waterline);
            const status = String(waterline.status ?? 'unknown');
            const totalBytes = Number(storage.total_bytes ?? 0);
            const targetCount = asArray(storage.targets).length;
            const alerts = asArray(waterline.alerts).length;
            vscode.window.showInformationMessage(`data.lifecycle: status=${status}, total_bytes=${totalBytes}, targets=${targetCount}, alerts=${alerts}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`data.lifecycle failed: ${getErrorMessage(error)}`);
        }
    });
    const optimizationPeakRpcCommand = vscode.commands.registerCommand('go-on.optimizationPeak', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('optimization.peak', {
                task: 'BLUE15 one-shot optimization peak',
                freeze_mode: 'strict'
            }));
            const peak = asRecord(result.peak);
            const gates = asArray(peak.gates);
            const passed = gates.filter((item) => asRecord(item).passed === true).length;
            const overallPass = peak.overall_pass === true;
            const status = String(peak.status ?? 'unknown');
            const version = String(peak.version ?? '-');
            vscode.window.showInformationMessage(`optimization.peak: status=${status}, overall_pass=${overallPass}, gates=${passed}/${gates.length}, version=${version}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`optimization.peak failed: ${getErrorMessage(error)}`);
        }
    });
    const autotuneStatusRpcCommand = vscode.commands.registerCommand('go-on.autotuneStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = await deps.sendRequest('autotune.status');
            vscode.window.showInformationMessage(`autotune.status: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`autotune.status failed: ${getErrorMessage(error)}`);
        }
    });
    const selectorStatusRpcCommand = vscode.commands.registerCommand('go-on.selectorStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('selector.status'));
            const mode = String(result.mode ?? 'unknown');
            const selector = asRecord(result.selector);
            const models = asArray(selector.models);
            const topModel = models.length > 0 ? asRecord(models[0]) : {};
            vscode.window.showInformationMessage(`selector.status: mode=${mode}, exploration_bias=${Number(selector.exploration_bias ?? 0).toFixed(2)}, tracked_models=${Number(selector.tracked_models ?? 0)}, total_observations=${Number(selector.total_observations ?? 0)}, top_model=${String(topModel.model_id ?? '-')}, top_score=${Number(topModel.ucb_score ?? 0).toFixed(3)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`selector.status failed: ${getErrorMessage(error)}`);
        }
    });
    const governanceStatusRpcCommand = vscode.commands.registerCommand('go-on.governanceStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('governance.status'));
            const governance = asRecord(result.governance);
            const governanceConfig = asRecord(governance.config);
            const rules = asRecord(governance.rules);
            const strictEnabled = governanceConfig.production_strict === true;
            const strictViolations = Number(governanceConfig.strict_violation_count ?? 0);
            const entryAuthEnabled = governanceConfig.entry_auth_enabled === true;
            const entryAuthKeyConfigured = governanceConfig.entry_auth_key_configured === true;
            const entryRateLimit = Number(governanceConfig.entry_rate_limit_rpm ?? 0);
            vscode.window.showInformationMessage(`governance=${governance.status ?? 'unknown'}, strict=${strictEnabled ? 'on' : 'off'}, strict_violations=${strictViolations}, entry_auth=${entryAuthEnabled ? 'on' : 'off'}, entry_key=${entryAuthKeyConfigured ? 'set' : 'missing'}, entry_rpm=${entryRateLimit}, rules=${rules.version ?? '-'}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`governance.status failed: ${getErrorMessage(error)}`);
        }
    });
    const governancePlanGetRpcCommand = vscode.commands.registerCommand('go-on.governancePlanGet', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('governance.plan.get'));
            const plan = asRecord(result.plan);
            const escalationLevel = String(plan.escalation_level ?? 'L1');
            const redLines = asArray(plan.red_lines).length;
            const stageReq = asArray(plan.stage_requirements).length;
            const safeguards = asArray(plan.mandatory_safeguards).length;
            vscode.window.showInformationMessage(`governance.plan.get: escalation=${escalationLevel}, red_lines=${redLines}, stage_requirements=${stageReq}, safeguards=${safeguards}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`governance.plan.get failed: ${getErrorMessage(error)}`);
        }
    });
    const governanceAuditRecentRpcCommand = vscode.commands.registerCommand('go-on.governanceAuditRecent', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const limitText = await vscode.window.showInputBox({
            prompt: 'Limit for governance.audit.recent',
            placeHolder: '20',
            value: '20'
        });
        if (limitText === undefined) {
            return;
        }
        const limit = Number.parseInt(limitText, 10);
        const safeLimit = Number.isFinite(limit) && limit > 0 ? Math.min(limit, 200) : 20;
        try {
            const result = asRecord(await deps.sendRequest('governance.audit.recent', { limit: safeLimit }));
            const audit = asRecord(result.audit);
            const events = asArray(audit.events);
            const latestRaw = events.length > 0 ? asRecord(events[events.length - 1]).action : '-';
            const latestAction = String(latestRaw ?? '-');
            vscode.window.showInformationMessage(`governance.audit.recent: events=${events.length}, latest_action=${latestAction}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`governance.audit.recent failed: ${getErrorMessage(error)}`);
        }
    });
    const autotuneGetRpcCommand = vscode.commands.registerCommand('go-on.autotuneGet', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = await deps.sendRequest('autotune.get');
            vscode.window.showInformationMessage(`autotune.get: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`autotune.get failed: ${getErrorMessage(error)}`);
        }
    });
    const autotuneResetRpcCommand = vscode.commands.registerCommand('go-on.autotuneReset', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const confirm = await vscode.window.showWarningMessage('Reset autotune state? This will clear learned parameters.', 'Reset', 'Cancel');
        if (confirm !== 'Reset') {
            return;
        }
        try {
            const result = await deps.sendRequest('autotune.reset', {});
            vscode.window.showInformationMessage(`autotune.reset: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`autotune.reset failed: ${getErrorMessage(error)}`);
        }
    });
    const metricsGetRpcCommand = vscode.commands.registerCommand('go-on.metricsGet', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const metrics = asRecord(await deps.sendRequest('metrics.get'));
            vscode.window.showInformationMessage(`metrics: chat=${Number(metrics.chat_requests_total ?? 0)}, failed=${Number(metrics.failed_requests ?? 0)}, agent_timeout=${Number(metrics.agent_timeout_failures_total ?? 0)}, review_timeout=${Number(metrics.review_gate_timeout_total ?? 0)}, probe_timeout=${Number(metrics.runtime_probe_timeout_total ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`metrics.get failed: ${getErrorMessage(error)}`);
        }
    });
    const metricsResetRpcCommand = vscode.commands.registerCommand('go-on.metricsReset', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const confirm = await vscode.window.showWarningMessage('Reset all runtime metric counters?', 'Reset', 'Cancel');
        if (confirm !== 'Reset') {
            return;
        }
        try {
            await deps.sendRequest('metrics.reset');
            vscode.window.showInformationMessage('Metrics reset.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`metrics.reset failed: ${getErrorMessage(error)}`);
        }
    });
    const traceMetricsRpcCommand = vscode.commands.registerCommand('go-on.traceMetrics', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const trace = asRecord(await deps.sendRequest('trace.metrics'));
            const timeouts = asRecord(trace.timeouts);
            const topN = asArray(trace.slow_requests_top_n).length;
            vscode.window.showInformationMessage(`trace.metrics: buffered=${Number(trace.buffered_events ?? 0)}, slow_top_n=${topN}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`trace.metrics failed: ${getErrorMessage(error)}`);
        }
    });
    const qualityBaselineRpcCommand = vscode.commands.registerCommand('go-on.qualityBaseline', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const healthResult = asRecord(await deps.sendRequest('runtime.health'));
            const metrics = asRecord(await deps.sendRequest('metrics.get'));
            const trace = asRecord(await deps.sendRequest('trace.metrics'));
            const lifecycle = asRecord(healthResult.lifecycle);
            const timeouts = asRecord(trace.timeouts);
            const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            let scenarioCount = 0;
            if (workspaceRoot) {
                const requestsDir = path.join(workspaceRoot, 'requests');
                if (fs.existsSync(requestsDir)) {
                    scenarioCount = fs
                        .readdirSync(requestsDir)
                        .filter((name) => name.toLowerCase().endsWith('.ndjson')).length;
                }
            }
            vscode.window.showInformationMessage(`quality.baseline: healthy=${Boolean(lifecycle.is_healthy)}, total=${Number(metrics.total_requests ?? 0)}, success=${Number(metrics.successful_requests ?? 0)}, failed=${Number(metrics.failed_requests ?? 0)}, avg_ms=${Number(metrics.avg_request_duration_ms ?? 0).toFixed(1)}, buffered=${Number(trace.buffered_events ?? 0)}, scenarios=${scenarioCount}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`quality.baseline failed: ${getErrorMessage(error)}`);
        }
    });
    const runtimeStabilityRpcCommand = vscode.commands.registerCommand('go-on.runtimeStability', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('runtime.stability'));
            const stability = asRecord(result.stability);
            const checks = asArray(stability.checks);
            const summary = asRecord(stability.summary);
            const checkSummary = checks
                .map((check) => {
                const checkEntry = asRecord(check);
                return `${String(checkEntry.name ?? '-')}=${String(checkEntry.status ?? '-')}`;
            })
                .join(', ');
            vscode.window.showInformationMessage(`runtime.stability: score=${Number(stability.score ?? 0)}, level=${stability.level ?? 'unknown'}, safe_restart=${Boolean(stability.safe_restart_ready)}, health_errors=${Number(summary.health_errors ?? 0)}, health_warnings=${Number(summary.health_warnings ?? 0)}, config_warnings=${Number(summary.config_warnings ?? 0)}, strict_violations=${Number(summary.strict_violations ?? 0)}, checks=[${checkSummary}]`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`runtime.stability failed: ${getErrorMessage(error)}`);
        }
    });
    const runtimeSelfModelRpcCommand = vscode.commands.registerCommand('go-on.runtimeSelfModel', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('runtime.self_model', { window: 120 }));
            const selfModel = asRecord(result.self_model);
            const health = asRecord(selfModel.health);
            const readiness = asRecord(health.readiness);
            const stability = asRecord(selfModel.stability);
            const drift = asRecord(selfModel.drift);
            const decision = asRecord(selfModel.decision);
            const recommendations = asArray(selfModel.recommendations);
            vscode.window.showInformationMessage(`runtime.self_model: readiness=${String(readiness.status ?? 'unknown')}, stability=${String(stability.level ?? 'unknown')}, safe_restart=${Boolean(stability.safe_restart_ready)}, mode=${String(decision.recommended_mode ?? 'unknown')}, drift_alert=${Boolean(drift.alert)}, drift_diff=${Number(drift.absolute_diff ?? 0).toFixed(4)}, recommendations=${recommendations.length}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`runtime.self_model failed: ${getErrorMessage(error)}`);
        }
    });
    const providerStatusRpcCommand = vscode.commands.registerCommand('go-on.providerStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('provider.status', {}));
            const providerStatus = asRecord(result.provider_status);
            const summary = asRecord(providerStatus.summary);
            vscode.window.showInformationMessage(`provider.status: status=${String(providerStatus.status ?? 'unknown')}, ready=${Number(summary.ready ?? 0)}, degraded=${Number(summary.degraded ?? 0)}, configured=${Number(summary.configured ?? 0)}, coverage=${Number(summary.coverage_percent ?? 0)}%`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`provider.status failed: ${getErrorMessage(error)}`);
        }
    });
    const releaseReadinessRpcCommand = vscode.commands.registerCommand('go-on.releaseReadiness', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('release.readiness', {}));
            const readiness = asRecord(result.readiness);
            const summary = asRecord(readiness.summary);
            vscode.window.showInformationMessage(`release.readiness: status=${String(readiness.status ?? 'unknown')}, overall=${Boolean(readiness.overall_pass)}, blocked=${Number(readiness.blocked_gate_count ?? 0)}, open_breakers=${Number(summary.open_breakers ?? 0)}, degraded_services=${Number(summary.degraded_services ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`release.readiness failed: ${getErrorMessage(error)}`);
        }
    });
    const harnessStatusRpcCommand = vscode.commands.registerCommand('go-on.harnessStatus', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('harness.status', { seed: 20260415 }));
            const harness = asRecord(result.harness);
            const suites = asRecord(harness.suites);
            const smoke = asRecord(suites.smoke);
            const regression = asRecord(suites.regression);
            const adversarial = asRecord(suites.adversarial);
            const longChain = asRecord(suites.long_chain);
            vscode.window.showInformationMessage(`harness.status: total=${Number(harness.scenario_total ?? 0)}, smoke=${Number(smoke.count ?? 0)}, regression=${Number(regression.count ?? 0)}, adversarial=${Number(adversarial.count ?? 0)}, long_chain=${Number(longChain.count ?? 0)}, seed=${Number(harness.fixed_seed ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`harness.status failed: ${getErrorMessage(error)}`);
        }
    });
    const traceGetRpcCommand = vscode.commands.registerCommand('go-on.traceGet', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = await deps.sendRequest('trace.get', {});
            vscode.window.showInformationMessage(`trace.get: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`trace.get failed: ${getErrorMessage(error)}`);
        }
    });
    const observabilityAlertsRpcCommand = vscode.commands.registerCommand('go-on.observabilityAlerts', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('observability.alerts', { limit: 20 }));
            const alerts = asRecord(result.alerts);
            const items = asArray(alerts.items);
            const topCode = items.length > 0 ? String(asRecord(items[0]).code ?? '-') : '-';
            vscode.window.showInformationMessage(`observability.alerts: critical=${Number(alerts.critical ?? 0)}, warn=${Number(alerts.warn ?? 0)}, info=${Number(alerts.info ?? 0)}, top=${topCode}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`observability.alerts failed: ${getErrorMessage(error)}`);
        }
    });
    const securityBaselineRpcCommand = vscode.commands.registerCommand('go-on.securityBaseline', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('security.baseline', {}));
            const baseline = asRecord(result.baseline);
            const productionStrict = asRecord(baseline.production_strict);
            const level = String(baseline.level ?? 'unknown');
            const ingress = String(baseline.ingress_status ?? 'unknown');
            const riskCount = Number(baseline.risk_count ?? 0);
            const strict = Boolean(productionStrict.enabled ?? false);
            vscode.window.showInformationMessage(`security.baseline: level=${level}, ingress=${ingress}, strict=${strict}, risks=${riskCount}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`security.baseline failed: ${getErrorMessage(error)}`);
        }
    });
    const breakerResetRpcCommand = vscode.commands.registerCommand('go-on.breakerReset', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const agent = await vscode.window.showInputBox({
            prompt: 'Agent name to reset circuit breaker for',
            placeHolder: 'e.g. copilot, deepseek, gemini'
        });
        if (!agent) {
            return;
        }
        try {
            const result = await deps.sendRequest('breaker.reset', { agent });
            vscode.window.showInformationMessage(`breaker.reset: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`breaker.reset failed: ${getErrorMessage(error)}`);
        }
    });
    const breakerRecoveryRpcCommand = vscode.commands.registerCommand('go-on.breakerRecovery', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const target = await vscode.window.showInputBox({
            prompt: 'Optional agent name to recover (leave empty for all degraded services)',
            placeHolder: 'e.g. copilot, deepseek, gemini'
        });
        if (target === undefined) {
            return;
        }
        try {
            const params = target.trim().length > 0 ? { agent: target.trim() } : {};
            const result = asRecord(await deps.sendRequest('breaker.recovery', params));
            const recoveredCount = Number(result.recovered_count ?? 0);
            const remaining = Number(result.remaining_degraded_count ?? 0);
            vscode.window.showInformationMessage(`breaker.recovery: recovered=${recoveredCount}, remaining_degraded=${remaining}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`breaker.recovery failed: ${getErrorMessage(error)}`);
        }
    });
    const maintenanceGcRpcCommand = vscode.commands.registerCommand('go-on.maintenanceGc', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            await deps.sendRequest('maintenance.gc');
            vscode.window.showInformationMessage('Maintenance GC completed.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`maintenance.gc failed: ${getErrorMessage(error)}`);
        }
    });
    const checkpointCreateRpcCommand = vscode.commands.registerCommand('go-on.checkpointCreate', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const conversationId = await vscode.window.showInputBox({
            prompt: 'Conversation ID',
            placeHolder: 'e.g. default-session'
        });
        if (!conversationId) {
            return;
        }
        const message = await vscode.window.showInputBox({
            prompt: 'Checkpoint message',
            placeHolder: 'Describe current conversation state'
        });
        if (!message) {
            return;
        }
        try {
            const result = await deps.sendRequest('conversation.checkpoint.create', {
                conversation_id: conversationId,
                messages: [{ role: 'user', content: message }],
            });
            vscode.window.showInformationMessage(`Checkpoint created: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`checkpoint.create failed: ${getErrorMessage(error)}`);
        }
    });
    const checkpointListRpcCommand = vscode.commands.registerCommand('go-on.checkpointList', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const conversationId = await vscode.window.showInputBox({
            prompt: 'Conversation ID',
            placeHolder: 'e.g. default-session'
        });
        if (!conversationId) {
            return;
        }
        try {
            const result = await deps.sendRequest('conversation.checkpoint.list', {
                conversation_id: conversationId,
            });
            vscode.window.showInformationMessage(`Checkpoints: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`checkpoint.list failed: ${getErrorMessage(error)}`);
        }
    });
    const conversationRollbackRpcCommand = vscode.commands.registerCommand('go-on.conversationRollback', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        const checkpointId = await vscode.window.showInputBox({
            prompt: 'Checkpoint ID to roll back to',
            placeHolder: 'e.g. ckpt-001'
        });
        if (!checkpointId) {
            return;
        }
        const conversationId = await vscode.window.showInputBox({
            prompt: 'Conversation ID',
            placeHolder: 'e.g. default-session'
        });
        if (!conversationId) {
            return;
        }
        try {
            const result = await deps.sendRequest('conversation.rollback', {
                conversation_id: conversationId,
                checkpoint_id: checkpointId,
            });
            vscode.window.showInformationMessage(`Rolled back: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`conversation.rollback failed: ${getErrorMessage(error)}`);
        }
    });
    const primarySecondarySummaryRpcCommand = vscode.commands.registerCommand('go-on.primarySecondarySummary', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = await deps.sendRequest('primary_secondary.summary', {});
            vscode.window.showInformationMessage(`primary_secondary.summary: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`primary_secondary.summary failed: ${getErrorMessage(error)}`);
        }
    });
    const debugPanelGetRpcCommand = vscode.commands.registerCommand('go-on.debugPanelGet', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('debug_panel.get', {}));
            const panel = asRecord(result.panel);
            const conversations = asRecord(panel.conversations);
            vscode.window.showInformationMessage(`debug_panel.get: conversations=${Number(conversations.count ?? 0)}, checkpoints=${Number(conversations.checkpoints ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`debug_panel.get failed: ${getErrorMessage(error)}`);
        }
    });
    const actionCheckRpcCommand = vscode.commands.registerCommand('go-on.actionCheck', async () => {
        if (!ensureRunning(deps)) {
            return;
        }
        try {
            const result = asRecord(await deps.sendRequest('action.check', { kind: 'all' }));
            const report = asRecord(result.report);
            vscode.window.showInformationMessage(`action.check: ok=${Boolean(result.ok)}, checks=${Number(report.total_checks ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`action.check failed: ${getErrorMessage(error)}`);
        }
    });
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
exports.registerRpcCommands = registerRpcCommands;
//# sourceMappingURL=rpcCommandRegistry.js.map