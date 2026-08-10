# go-on-sdk

TypeScript SDK for the [go-on](https://github.com/go-on/go-on) ACP/MCP agent orchestration runtime.

## Installation

```bash
npm install go-on-sdk
```

## Quick Start

```typescript
import { GoOnClient, ChatMessage } from "go-on-sdk";

const client = new GoOnClient("http://127.0.0.1:8090");

// Check health
const health = await client.health();
console.log("Health:", health);

// Send a chat message (blocking)
const response = await client.runtimeHealth();
console.log("Runtime health:", response);

// Stream a chat response
const messages: ChatMessage[] = [{ role: "user", content: "Hello!" }];
for await (const chunk of client.chatStream({ messages })) {
  console.log("Chunk:", chunk);
}
```

## API

### GoOnClient

| Method               | JSON-RPC Method            | Description                      |
|----------------------|----------------------------|----------------------------------|
| `health()`           | `GET /health`              | Quick health check               |
| `runtimeHealth()`    | `runtime.health`           | Full runtime health              |
| `runtimeStability()` | `runtime.stability`        | Runtime stability snapshot       |
| `initialize()`       | `initialize`               | Initialize the runtime (`setupLevel` optional/reserved) |
| `shutdown()`         | `shutdown`                 | Gracefully shut down             |
| `governanceStatus()` | `governance.status`        | Governance status                |
| `healthProbes()`     | `health.probes`            | Module-level health probes       |
| `metricsGet()`       | `metrics.get`              | Runtime metrics                  |
| `metricsPrometheus()`| `GET /metrics`            | Prometheus-formatted metrics (plain text) |
| `breakerStatus()`    | `breaker.status`           | Circuit breaker status           |
| `checkpointList()`   | `checkpoint.list`          | List checkpoints                 |
| `workflowExecute()`  | `workflow.execute`         | Execute workflow                 |
| `chatStream()`       | `POST /chat/stream`       | Streaming chat (SSE)             |

## License

MIT
