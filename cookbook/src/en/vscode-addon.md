# VS Code Addon

The VS Code addon is the richest editor integration in this repository. It exposes runtime-backed commands, can probe runtime health, and can override the backend transport mode from extension settings.

## What the addon expects

The extension needs:

- a reachable `go-on` executable
- a valid `config.toml`
- a runtime mode that matches the selected workflow

The addon manifest currently exposes these protocol override values:

- `from_config`
- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

`from_config` keeps the extension aligned with the backend config. The others force an explicit override.

## Recommended first-time setup

1. Build the backend or use an existing binary.
2. Run `go-on --setup --setup-level standard`.
3. In VS Code, configure the executable path and config path if auto-discovery is insufficient.
4. Keep `protocolMode` on `from_config` unless you are debugging a specific transport problem.

## When to use each protocol mode

- `from_config`: default choice for normal daily use.
- `adaptive`: preferred override when you want one runtime that can satisfy mixed probes.
- `acp_stdio`: use when the extension should drive a spawned stdio runtime contract.
- `acp_http`: use when the backend is already running as a shared local HTTP service.
- `mcp_stdio`: only when your extension workflow explicitly requires MCP over stdio.
- `mcp_http`: use when you want explicit `/v1` HTTP semantics.

## Runtime health expectations

The extension contract expects the runtime health path at:

```text
/health
```

The OpenAI-compatible probe path is:

```text
/v1/models
```

The addon also knows about:

- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

## Practical workspace settings pattern

Example workspace-level settings:

```json
{
  "go-on.runtime.protocolMode": "from_config",
  "go-on.executablePath": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
  "go-on.configPath": "D:/Workspace/RustWorkspace/go-on/config.toml"
}
```

When you want to force a shared HTTP runtime:

```json
{
  "go-on.runtime.protocolMode": "acp_http"
}
```

## Operational workflow

For the addon, a good debugging sequence is:

1. `go-on --validate-config`
2. `go-on --status`
3. confirm the executable path in VS Code settings
4. confirm the config path in VS Code settings
5. only then override the protocol mode if required

## When to use HTTP instead of stdio

Prefer HTTP when:

- you want the GUI and VS Code to target the same backend instance
- you want manual probing through `/health` and `/v1/models`
- you want long-running local service behavior

Prefer stdio when:

- you want VS Code to own process startup and shutdown
- you want each workspace session isolated from the others

## Common failure patterns

- If the addon can spawn the executable but reports provider not ready, the issue is configuration or credentials, not transport.
- If HTTP mode is selected and `/health` is unreachable, the backend was not started with `--acp-http-bind`.
- If forcing `mcp_http`, make sure the consuming feature really expects the `/v1` surface rather than ACP semantics.

## Available commands

The addon registers the following commands in the VS Code command palette:

**Lifecycle**

| Command | Description |
|---|---|
| `go-on.start` | Start the Go-On backend process |
| `go-on.stop` | Stop the running backend process |
| `go-on.shutdown` | Gracefully shut down the backend |
| `go-on.healthCheck` | Check runtime health |
| `go-on.healthProbes` | View all health probe details |

**Runtime diagnostics**

| Command | Description |
|---|---|
| `go-on.runtimeSelfModel` | Get the unified self-model view: health, drift summary, constraints, and suggested actions |
| `go-on.runtimeStability` | Get runtime stability snapshot |
| `go-on.providerStatus` | Get provider readiness, degradation status, and agent dependency snapshot |
| `go-on.metricsGet` | Get current runtime metrics |
| `go-on.metricsReset` | Reset runtime metrics |
| `go-on.traceMetrics` | Get trace-level metrics |
| `go-on.traceGet` | Get trace entries |
| `go-on.observabilityAlerts` | View observability alerts |
| `go-on.releaseReadiness` | Check release readiness gate |

**Governance & quality**

| Command | Description |
|---|---|
| `go-on.governanceStatus` | Get governance status |
| `go-on.governancePlanGet` | Get active governance plan |
| `go-on.governanceAuditRecent` | View recent audit entries |
| `go-on.qualityBaseline` | Get quality baseline snapshot |
| `go-on.securityBaseline` | Get security baseline |
| `go-on.rlAlignmentEval` | Run RL alignment offline evaluation |
| `go-on.hardnessStatus` | Get task hardness status |
| `go-on.costStatus` | Get cost optimization status |
| `go-on.autotuneStatus` | Get autotune status |
| `go-on.autotuneGet` | Get autotune parameters |
| `go-on.autotuneReset` | Reset autotune parameters |
| `go-on.selectorStatus` | Get model selector status |

**Workflow & tasks**

| Command | Description |
|---|---|
| `go-on.workflowExecute` | Execute the current workflow |
| `go-on.taskPlan` | Plan a task |
| `go-on.taskExecute` | Execute a planned task |
| `go-on.harnessStatus` | Get test harness status |
| `go-on.primarySecondarySummary` | Get primary/secondary agent summary |

**Learning & optimization**

| Command | Description |
|---|---|
| `go-on.learningSummary` | Get learning loop summary |
| `go-on.learningGuardrail` | Get learning guardrail status |
| `go-on.learningReplay` | Replay learning data |
| `go-on.knowledgeDistill` | Run knowledge distillation |
| `go-on.optimizationPeak` | Get optimization peak status |
| `go-on.buildRepro` | Run build reproducibility check |

**Config & maintenance**

| Command | Description |
|---|---|
| `go-on.configReload` | Reload the runtime config |
| `go-on.configBaseline` | Get config baseline snapshot |
| `go-on.lockStatus` | Get lock status |
| `go-on.breakerStatus` | Get circuit breaker status |
| `go-on.breakerReset` | Reset circuit breaker |
| `go-on.breakerRecovery` | Run circuit breaker recovery |
| `go-on.cacheClear` | Clear the ACP cache |
| `go-on.vectorClear` | Clear the vector store |
| `go-on.dataLifecycle` | Get data lifecycle status |
| `go-on.errorContract` | Get error contract summary |
| `go-on.checkpointCreate` | Create a runtime checkpoint |
| `go-on.checkpointList` | List available checkpoints |
| `go-on.conversationRollback` | Roll back to a checkpoint |
| `go-on.maintenanceGc` | Run garbage collection |
| `go-on.actionCheck` | Check action safety |
| `go-on.debugPanelGet` | Get debug panel data |

## Process output channel

All Go-On process output (stdout, stderr, exit codes, process errors) is written to the **"Go-On"** Output Channel in VS Code. Open it from **View → Output**, then select **Go-On** from the dropdown. This is the primary diagnostic surface for startup failures and runtime errors.