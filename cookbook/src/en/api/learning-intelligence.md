# Learning and Intelligence API

## Overview

The Learning and Intelligence API exposes machine learning, reinforcement learning, adaptive selection, and knowledge distillation capabilities of go-on. The API is **JSON-RPC 2.0 over HTTP** (`POST /rpc`); there are no dedicated REST endpoints for these capabilities.

> The authoritative JSON-RPC method reference lives in `docs/protocol-guide.md`.

## Methods

All methods are dispatched via `POST /rpc`:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"learning.summary","params":{}}'
```

### Learning & Knowledge

| Method | Description |
|---|---|
| `learning.summary` | Learning profile summary for a task window |
| `learning.replay` | Learning replay profile |
| `learning.guardrail` | Learning guardrail summary (window/limit params) |
| `knowledge.distill` | Distill knowledge (evidence-weighted extraction, write-back to `learning.summary` / `knowledge.distill`) |

### Reinforcement Learning & Adaptive Selection

| Method | Description |
|---|---|
| `rl.alignment.offline_eval` | Offline evaluation of RL alignment |
| `selector.status` | Model/tool selector status |
| `phase.policy.replay` | Phase policy replay |
| `primary_secondary.summary` | Primary/secondary summary (alias: `summary/primary_secondary`) |
| `optimization.peak` | Optimization peak analysis |
| `cost.status` | Cost status |

### Supporting Intelligence

| Method | Description |
|---|---|
| `harness.status` | Harness status with learning/rl profile integration |
| `capabilities.list` | Server capability list |
| `models.list` / `models/list` | Available models |

## Authentication

All methods require authentication with appropriate permissions (RBAC is enforced per request).

## Next Steps

- Explore [Safety and Governance API](./safety-governance.md)
- Check [Workflow and Task API](./workflow-task.md)
- See [Optimization and Operations API](./optimization-ops.md)
