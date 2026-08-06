# Safety and Governance API

## Overview

The Safety and Governance API enables security policy enforcement, audit trail maintenance, compliance monitoring, and access control for go-on deployments. The API is **JSON-RPC 2.0 over HTTP** (`POST /rpc`); there are no dedicated REST endpoints for these capabilities.

> The backend JSON-RPC dispatch table lives in `src/acp/impl/request.rs`; the method allowlist is in `src/acp/impl/request/protocol.rs`. `docs/protocol-guide.md` covers protocol modes only.

## Methods

All methods are dispatched via `POST /rpc`:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"governance.status","params":{}}'
```

### Governance

| Method | Description |
|---|---|
| `governance.status` | Governance status (HarnessBus profile, policies, gates) |
| `governance.plan.get` | Get the governance plan |
| `governance.plan.update` | Update the governance plan |
| `governance.audit.recent` | Recent audit log entries |
| `governance.audit.verify` | Verify the tamper-evident audit hash chain |
| `governance.remediate` | Run governance remediation |
| `governance.config.save` | Save governance configuration |

### Security

| Method | Description |
|---|---|
| `security.baseline` | Security baseline and risk report |
| `harness.status` | HarnessBus status (policy, drift, resilience, audit dimensions) |
| `tool.approve` | Approve a tool for execution (params: `tool_name`) |

### Access Control

Authentication and RBAC are enforced per request:

- `authenticate` — authenticate a session
- `logout` — end a session
- RBAC maps each method to a permission level (`Admin`, `ManageUsers`, `ManageConfig`, `Read`, `Execute`); sensitive methods (`shutdown`, `maintenance.gc`) require admin privileges

## Audit Trail

Configuration changes and maintenance operations are recorded in the audit log, and the audit hash chain can be verified via `governance.audit.verify`.

## Next Steps

- Explore [Core Runtime API](./core-runtime.md)
- See [Optimization and Operations API](./optimization-ops.md)
- Review [Observability API](./observability.md)
