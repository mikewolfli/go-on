# Go-On VS Code Extension

VS Code extension for operating and interacting with go-on runtime.

## Version

- Extension: 1.6.0
- Target runtime: go-on 1.6.0

## What Is Implemented (Current)

- Runtime process start/stop and status monitoring
- Chat panel and chat command entry
- Settings panel and process status panel
- Workflow and process-flow webviews (local persisted definitions)
- Advanced edit/refactor commands (chat-driven generation)
- Runtime download and path configuration support

## Command To Backend Mapping

The following commands are wired to concrete backend RPC methods (verified against `package.json` `contributes.commands`, 85 commands, and `src/` registries):

| VS Code command | Runtime RPC method |
|---|---|
| `go-on.sendRequest` | any method (prompts; aliases `chat.delete`→`session/delete`, `session.clear`→`session/delete`, `memory.clear`→`vector.clear`) |
| `go-on.healthCheck` | `runtime.health` |
| `go-on.healthProbes` | `health.probes` |
| `go-on.diagnose` | `runtime.health` (as part of the diagnosis report) |
| `go-on.breakerStatus` | `breaker.status` |
| `go-on.cacheClear` | `cache.clear` |
| `go-on.vectorClear` | `vector.clear` |
| `go-on.configReload` | `config.reload` |
| `go-on.shutdown` | `shutdown` |
| `go-on.stop` | process stop (no RPC) |
| `go-on.lockStatus` | `lock.status` |
| `go-on.editCode` | `chat` |
| `go-on.refactorCode` | `chat` |
| `go-on.workflowExecute` | `workflow.execute` |
| `go-on.taskPlan` | `task.plan` |
| `go-on.taskExecute` | `task.execute` |
| `go-on.learningSummary` | `learning.summary` |
| `go-on.autotuneStatus` | `autotune.status` |
| `go-on.autotuneGet` | `autotune.get` |
| `go-on.autotuneReset` | `autotune.reset` |
| `go-on.learningGuardrail` | `learning.guardrail` |
| `go-on.learningReplay` | `learning.replay` |
| `go-on.knowledgeDistill` | `knowledge.distill` |
| `go-on.rlAlignmentEval` | `rl.alignment.offline_eval` |
| `go-on.hardnessStatus` | `hardness.status` |
| `go-on.costStatus` | `cost.status` |
| `go-on.configBaseline` | `config.baseline` |
| `go-on.errorContract` | `error.contract` |
| `go-on.buildRepro` | `build.repro` |
| `go-on.dataLifecycle` | `data.lifecycle` |
| `go-on.optimizationPeak` | `optimization.peak` |
| `go-on.runtimeSelfModel` | `runtime.self_model` |
| `go-on.providerStatus` | `provider.status` |
| `go-on.releaseReadiness` | `release.readiness` |
| `go-on.runtimeStability` | `runtime.stability` |
| `go-on.governanceStatus` | `governance.status` |
| `go-on.governancePlanGet` | `governance.plan.get` |
| `go-on.governanceAuditRecent` | `governance.audit.recent` |
| `go-on.governanceAuditVerify` | `governance.audit.verify` |
| `go-on.skillListImported` | `skill.list_imported` |
| `go-on.skillImportLocal` | `skill.import` |
| `go-on.skillToggle` | `skill.enable` / `skill.disable` / `skill.remove` (prompted) |
| `go-on.selectorStatus` | `selector.status` |
| `go-on.qualityBaseline` | `runtime.health` + `metrics.get` + `trace.metrics` |
| `go-on.metricsGet` | `metrics.get` |
| `go-on.metricsReset` | `metrics.reset` |
| `go-on.traceMetrics` | `trace.metrics` |
| `go-on.harnessStatus` | `harness.status` |
| `go-on.traceGet` | `trace.get` |
| `go-on.observabilityAlerts` | `observability.alerts` |
| `go-on.securityBaseline` | `security.baseline` |
| `go-on.breakerReset` | `breaker.reset` |
| `go-on.breakerRecovery` | `breaker.recovery` |
| `go-on.maintenanceGc` | `maintenance.gc` |
| `go-on.checkpointCreate` | `conversation.checkpoint.create` |
| `go-on.checkpointList` | `checkpoint.list` |
| `go-on.conversationRollback` | `conversation.rollback` |
| `go-on.primarySecondarySummary` | `primary_secondary.summary` |
| `go-on.debugPanelGet` | `debug.panel.get` |
| `go-on.actionCheck` | `action.check` |
| `go-on.sendSelection` | `chat` |
| `go-on.sendFile` | `chat` |
| `go-on.workspaceContext` | `chat` |

The following are extension/UI behaviors (no dedicated runtime RPC):

- `go-on.start`, `go-on.stop`, `go-on.diagnose`
- `go-on.openChat`, `go-on.closeChat`, `go-on.openSettings`, `go-on.openConfigWizard`
- `go-on.newSession`, `go-on.switchSession`, `go-on.clearChat`, `go-on.exportChat`
- `go-on.createWorkflow`, `go-on.runWorkflow`, `go-on.showProcessFlow`
- `go-on.semanticSearch` (extension-side embedding search)
- `go-on.keyringSet`, `go-on.keyringGet`, `go-on.keyringDelete`, `go-on.keyringList`
- `go-on.applyDefaultConfigTemplate`, `go-on.updateWorkflowMapping`, `go-on.updateRules`
- `go-on.refreshStatusMonitor`, `go-on-status.refresh`

Notes about workflow/process views:

- Definitions are stored in extension global state.
- Chat-type steps call runtime `chat`.
- Non-chat steps are handled in extension-side logic.

## Available Commands (Contributed)

Full list registered in `package.json` `contributes.commands` (85 commands — the authoritative source; regenerate with a JSON scan of `package.json`):

- `go-on.start`, `go-on.stop`, `go-on.sendRequest`, `go-on.healthCheck`, `go-on.healthProbes`, `go-on.diagnose`
- `go-on.breakerStatus`, `go-on.cacheClear`, `go-on.vectorClear`, `go-on.configReload`, `go-on.shutdown`
- `go-on.lockStatus`, `go-on.selectorStatus`, `go-on.autotuneStatus`, `go-on.autotuneGet`, `go-on.autotuneReset`
- `go-on.openChat`, `go-on.closeChat`, `go-on.openSettings`, `go-on.openConfigWizard`
- `go-on.clearChat`, `go-on.exportChat`, `go-on.newSession`, `go-on.switchSession`
- `go-on.createWorkflow`, `go-on.runWorkflow`, `go-on.showProcessFlow`
- `go-on.editCode`, `go-on.refactorCode`, `go-on.sendSelection`, `go-on.sendFile`, `go-on.semanticSearch`, `go-on.workspaceContext`
- `go-on.workflowExecute`, `go-on.taskPlan`, `go-on.taskExecute`
- `go-on.learningSummary`, `go-on.learningGuardrail`, `go-on.learningReplay`, `go-on.knowledgeDistill`, `go-on.rlAlignmentEval`, `go-on.runtimeSelfModel`, `go-on.providerStatus`, `go-on.harnessStatus`
- `go-on.hardnessStatus`, `go-on.costStatus`, `go-on.configBaseline`, `go-on.errorContract`, `go-on.buildRepro`, `go-on.dataLifecycle`, `go-on.optimizationPeak`, `go-on.releaseReadiness`, `go-on.runtimeStability`
- `go-on.governanceStatus`, `go-on.governancePlanGet`, `go-on.governanceAuditRecent`, `go-on.governanceAuditVerify`
- `go-on.skillListImported`, `go-on.skillImportLocal`, `go-on.skillToggle`
- `go-on.qualityBaseline`, `go-on.metricsGet`, `go-on.metricsReset`, `go-on.traceMetrics`, `go-on.traceGet`, `go-on.observabilityAlerts`, `go-on.securityBaseline`
- `go-on.breakerReset`, `go-on.breakerRecovery`, `go-on.maintenanceGc`
- `go-on.checkpointCreate`, `go-on.checkpointList`, `go-on.conversationRollback`, `go-on.primarySecondarySummary`, `go-on.debugPanelGet`, `go-on.actionCheck`
- `go-on.keyringSet`, `go-on.keyringGet`, `go-on.keyringDelete`, `go-on.keyringList`
- `go-on.applyDefaultConfigTemplate`, `go-on.updateWorkflowMapping`, `go-on.updateRules`
- `go-on.refreshStatusMonitor`, `go-on-status.refresh`

## Settings

Key settings include:

- `go-on.configPath`
- `go-on.executablePath` (recommended: set to a local absolute path)
- `go-on.autoDownloadBinary` (default: false)
- `go-on.releaseRepository`
- `go-on.releaseTag`
- `go-on.autoStart`
- `go-on.runtime.protocolMode` (`from_config` / `adaptive` / `acp_stdio` / `acp_http` / `mcp_stdio` / `mcp_http`)
- `go-on.chat.*`
- `go-on.cache.enabled`
- `go-on.vector.enabled`
- `go-on.health.interval`
- `go-on.ui.*`
- `go-on.advancedAI.*`

When `go-on.runtime.protocolMode` is not `from_config`, the extension starts runtime with `--protocol-mode <value>` so protocol switching is applied on the main startup chain.

Error Trace Alignment (BLUE14 AGENT4)

- Runtime JSON-RPC errors are surfaced as `rpc_error:<code>:<kind>:<message> (context=<...>)`.
- `kind` is aligned with backend governance classes: `PuaViolation`, `BudgetExceeded`, `SandboxBlocked`.
- When available, context includes the backend dispatch prefix `acp.handle_request.dispatch` for fast diagnosis.

## Development

```bash
cd vscode-addon
npm install
npm run compile
```

## Quick Start

1. Install the extension — it activates automatically on VS Code startup (`onStartupFinished`).
2. Open the Go-On Chat view from the activity bar or command palette (`go-on.openChat`).
3. On first use, the extension will:
   - Auto-create a config file at `./config.toml` in the workspace root (or the path set in `go-on.configPath`)
   - Prompt to download the go-on runtime or select a local binary
   - Guide you through provider API key setup
4. Set `go-on.executablePath` and `go-on.configPath` in settings if you have a custom runtime.
5. `go-on.runtime.protocolMode` supports: `from_config` / `adaptive` / `acp_stdio` / `acp_http` / `mcp_stdio` / `mcp_http`.
6. Use `go-on.start` to start the runtime or let the chat view start it automatically.


## Sync Policy

Extension docs must stay aligned with actual implemented commands and runtime RPC wiring in `vscode-addon/src`.
