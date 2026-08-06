# Workflow and Task API

## Overview

The Workflow and Task API enables orchestration of complex workflows, task planning, execution management, and result tracking. The API is **JSON-RPC 2.0 over HTTP** (`POST /rpc`); there are no dedicated REST endpoints for these capabilities.

> The authoritative JSON-RPC method reference lives in `docs/protocol-guide.md`.

## Methods

All methods are dispatched via `POST /rpc`:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"task.plan","params":{}}'
```

### Workflows

| Method | Description |
|---|---|
| `workflow.execute` | Execute a workflow |
| `workflow.generate` | Generate a workflow from a prompt |
| `workflow.generate_from_chat` | Generate a workflow from the current chat context |
| `workflow.confirm` | Confirm a workflow step |
| `workflow.clarify` | Request clarification during a workflow |
| `workflow.research` | Run a research step |
| `workflow.consult` | Consult during workflow execution |
| `workflow.ask` | Ask a question during workflow execution |
| `workflow.run.list` | List workflow runs |
| `workflow.run.get` | Get a workflow run by ID |
| `workflow.run.cancel` | Cancel a workflow run |
| `workflow.run.pause` | Pause a workflow run |
| `workflow.run.resume` | Resume a workflow run |

### Tasks

| Method | Description |
|---|---|
| `task.plan` | Plan a task (controlled task plan artifact) |
| `task.execute` | Execute a task |
| `action.check` | Run action checks (all/spec/qa/retest/final) against `.goon/` artifacts |

## Authentication

All methods require authentication with appropriate permissions (RBAC is enforced per request).

## Next Steps

- Explore [Learning and Intelligence API](./learning-intelligence.md)
- See [Optimization and Operations API](./optimization-ops.md)
- Review [Safety and Governance API](./safety-governance.md)
