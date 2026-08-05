# go-on Node.js SDK

Asynchronous JSON-RPC client for the [go-on](https://github.com/mikewolfli/go-on) ACP agent orchestration runtime. Supports streaming chat, governance, observability, reliability, workflow, learning, and operations APIs.

## Installation

```bash
npm install go-on-sdk
```

## Quick Start

```ts
import { GoOnClient } from "go-on-sdk";

const client = new GoOnClient({ baseUrl: "http://127.0.0.1:8090" });

// Health check
const health = await client.health();
console.log(`Status: ${health.status}`); // "ok"

// Governance status
const governance = await client.governanceStatus();
console.log(`Governance OK: ${governance.ok}`);

// Streaming chat
for await (const chunk of client.chatStream([
  { role: "user", content: "Hello!" },
])) {
  process.stdout.write(chunk);
}
process.stdout.write("\n");

client.close();
```

`chatStream(messages, options?)` is a **true streaming** async generator over
SSE at `/chat/stream`: it yields each content token as it arrives over the
wire (`event: chunk` frames, preferring the `token` field, falling back to
legacy `content`/`text`). Frames without a text field (e.g. `telemetry`,
`done`) are yielded as their raw JSON payload so nothing is silently dropped.
`error` events throw a `GoOnClientError`; the generator terminates on the
`[DONE]` sentinel, on the terminal `done`/`result` event, or when the server
closes the connection.

## API Reference

### Runtime
| Method | RPC | Description |
|--------|-----|-------------|
| `health()` | `runtime.health` | Backend health status |
| `runtimeHealth()` | `health.probes` | Detailed module health |
| `runtimeStability()` | `runtime.stability` | Stability metrics |
| `initialize(profile)` | `initialize` | ACP initialize handshake |
| `shutdown()` | `shutdown` | Graceful shutdown |

### Governance
| Method | RPC | Description |
|--------|-----|-------------|
| `governanceStatus()` | `governance.status` | HarnessBus profile + policy stats |
| `governancePlanGet(id)` | `governance.plan.get` | Governance plan by ID |
| `governanceAuditRecent(n)` | `governance.audit.recent` | Recent audit entries |
| `governanceAuditVerify(params)` | `governance.audit.verify` | Verify the tamper-evident audit hash chain (optional `fromMs`/`toMs` report, `publicKeyHex` signature check) |

### Observability
| Method | RPC | Description |
|--------|-----|-------------|
| `healthProbes()` | `health.probes` | All module probes |
| `metricsGet()` | `metrics.get` | Structured metrics |
| `metricsPrometheus()` | `metrics.prometheus` | Prometheus format |
| `traceGet(id)` | `trace.get` | OTel trace by ID |

### Reliability
| Method | RPC | Description |
|--------|-----|-------------|
| `breakerStatus()` | `breaker.status` | Circuit breaker states |
| `breakerReset(name)` | `breaker.reset` | Reset a breaker by name |
| `maintenanceGc()` | `maintenance.gc` | Trigger GC |

### Checkpoint & Recovery
| Method | RPC | Description |
|--------|-----|-------------|
| `checkpointCreate(conversationId, messages, branch?)` | `conversation.checkpoint.create` | Create checkpoint for a conversation |
| `checkpointList()` | `checkpoint.list` | List checkpoints |
| `conversationRollback(id)` | `conversation.rollback` | Rollback conversation |

### Workflow / Task
| Method | RPC | Description |
|--------|-----|-------------|
| `workflowExecute(name, params?)` | `workflow.execute` | Execute workflow |
| `taskPlan(description)` | `task.plan` | Plan a task |
| `taskExecute(planId)` | `task.execute` | Execute task |

### Learning & Intelligence
| Method | RPC | Description |
|--------|-----|-------------|
| `learningSummary()` | `learning.summary` | RL + learning stats |
| `selectorStatus()` | `selector.status` | Agent selector state |
| `knowledgeDistill()` | `knowledge.distill` | Trigger distillation |
| `rlAlignmentOfflineEval()` | `rl.alignment.offline_eval` | RL eval |

### Optimization & Operations
| Method | RPC | Description |
|--------|-----|-------------|
| `costStatus()` | `cost.status` | Model cost breakdown |
| `configBaseline()` | `config.baseline` | Config snapshot |
| `configReload()` | `config.reload` | Hot-reload config |
| `harnessStatus()` | `harness.status` | Governance status |

## Streaming Chat

`chatStream(messages, options?)` returns an `AsyncGenerator<string>` that
streams tokens from the backend's SSE endpoint. See the Quick Start for a
usage example and the notes below it for the exact yield semantics.

## Error Handling

```ts
import { GoOnClientError, GoOnJsonRpcError, GoOnHttpError } from "go-on-sdk";

try {
  await client.health();
} catch (err) {
  if (err instanceof GoOnJsonRpcError) {
    console.error(`RPC error [${err.code}]: ${err.messageText}`);
  } else if (err instanceof GoOnHttpError) {
    console.error(`HTTP ${err.statusCode}: ${err.statusText}`);
  } else if (err instanceof GoOnClientError) {
    console.error(`Client error: ${err.message}`);
  }
}
```

## License

MIT
