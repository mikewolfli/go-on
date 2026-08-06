# Optimization and Operations API

## Overview

The Optimization and Operations API enables cost management, performance optimization, operational monitoring, and system tuning for go-on deployments. The API is **JSON-RPC 2.0 over HTTP** (`POST /rpc`); there are no dedicated REST endpoints for these capabilities.

> The backend JSON-RPC dispatch table lives in `src/acp/impl/request.rs`; the method allowlist is in `src/acp/impl/request/protocol.rs`. `docs/protocol-guide.md` covers protocol modes only.

## Methods

All methods are dispatched via `POST /rpc`:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"cost.status","params":{}}'
```

### Cost Optimization

| Method | Description |
|---|---|
| `cost.status` | Cost status |
| `optimization.peak` | Optimization peak analysis |
| `observability.alerts` | Observability alerts |

### Performance & Metrics

| Method | Description |
|---|---|
| `metrics.get` | Structured runtime metrics |
| `metrics` | Metrics payload |
| `metrics.prometheus` | Prometheus-format metrics |
| `metrics.window.query` | Query a metrics window |
| `metrics.errors.summary` | Error summary |
| `metrics.reset` | Reset metrics |
| `runtime.stability` | Runtime stability metrics |
| `trace.get` / `trace.metrics` | Trace inspection |
| `error.contract` | Error contract payload |

### Operations

| Method | Description |
|---|---|
| `breaker.status` / `breaker.reset` / `breaker.recovery` | Circuit breaker management |
| `lock.status` | ACP lock status |
| `maintenance.gc` | Maintenance garbage collection |
| `data.lifecycle` | Data lifecycle review |
| `cache.clear` / `vector.clear` | Clear cache / vector store |
| `autotune.get` / `autotune.status` / `autotune.reset` | Autotune management |
| `release.readiness` | Release readiness check |
| `harness.status` | Harness status (QA/reliability dimensions) |
| `hardness.status` | Hardness status |

## Next Steps

- Explore [Core Runtime API](./core-runtime.md)
- See [Observability API](./observability.md)
- Review [Safety and Governance API](./safety-governance.md)
