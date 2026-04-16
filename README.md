# go-on

English | [简体中文](README.zh-CN.md)

go-on is a Rust runtime for ACP/MCP-oriented agent orchestration, governance, and production-safe operations.

## Version

- Core runtime: 0.6.1
- GUI desktop: 0.6.1
- VS Code addon: 0.6.1
- Default feature: `local-acp-sqlite`
- Optional feature scaffold: `server-mcp-postgres`

## Repository Layout (Current)

Top-level key directories:

- `src/`: backend runtime implementation
- `GUI/`: Tauri + Vue desktop console
- `vscode-addon/`: VS Code extension
- `requests/`: NDJSON scenario benchmarks and replay inputs
- `scripts/`: quality/release gate scripts
- `deploy/nginx/`: ingress and TLS reverse-proxy templates
- `tests/`, `test_i18n/`: integration and i18n test suites
- `languages/`: runtime i18n resources
- `RULES/`: governance and coding rule packs

Backend source modules under `src/`:

- `acp`: ACP server, request routing, workflow/task/chat handling
- `agents`: provider adapters and agent contracts
- `core`: config, setup, readiness, errors
- `governance`: policy/rule controls and audit support
- `intelligence`: selectors, reinforcement, quality models
- `optimization`: cost/speed/reliability/failure-prevention
- `orchestration`: flow/mode/router/tool orchestration
- `observability`: metrics/trace/perf helpers
- `memory`: cache/vector stores
- `protocol`: protocol server and JSON-RPC support
- `mcp`, `i18n`: MCP adapter helpers and language runtime

## Runtime Modes

`[protocol].mode` supports 5 values:

- `adaptive` (recommended default)
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

Example:

```toml
[protocol]
mode = "adaptive"
```

## Quick Start

### 1) Build and Test

```bash
cargo build
cargo check --all-targets
cargo test --all-targets
```

### 2) First-time Setup

```bash
cargo run -- --init --config config.toml
cargo run -- --check --config config.toml
```

Optional setup levels:

```bash
cargo run -- --init --setup-level quick --config config.toml
cargo run -- --init --setup-level standard --config config.toml
cargo run -- --init --setup-level custom --config config.toml
```

### 3) Start Runtime

- Linux/macOS: `./start-go-on.sh`
- Windows: `start-go-on.bat`

Default health endpoint:

- `http://127.0.0.1:8090/health`

## Production Baseline

Production-oriented template:

- `config.production.toml`

Current baseline includes:

- loopback bind by default
- entry auth + entry rate limiting options
- strict production fail-fast (`runtime.production_strict = true`)
- OTEL-related runtime settings

### API Key Setup (production mode)

When running with `config.production.toml`, entry authentication is enabled by default.
Set the following environment variable **before** starting the server:

```bash
# Linux / macOS
export GO_ON_ENTRY_API_KEY="your-secret-key-here"
./start-go-on.sh
```

```powershell
# Windows
$env:GO_ON_ENTRY_API_KEY = "your-secret-key-here"
.\start-go-on.bat
```

All RPC requests must include the key in the `Authorization` header:

```
Authorization: Bearer your-secret-key-here
```

If this variable is missing or empty the server will reject all requests with error code `-32003` (`AuthRequired`).

> **Security**: Never commit secret values to version control. Use environment variables, a secrets manager, or a keyring-backed injector.

Ingress and TLS templates:

- `deploy/nginx/go-on.conf`
- `deploy/nginx/README.md`

Release readiness checklist:

- `RELEASE_READINESS.md`

## Scenario and Gate Tooling

Scenario replay assets are in `requests/` (runtime health, governance, cost, harness, security, release drill, etc.).

Gate scripts:

- `scripts/run-quality-gate.sh`
- `scripts/run-quality-gate.ps1`
- `scripts/run-release-readiness-gate.sh`
- `scripts/run-release-readiness-gate.ps1`
- `test_ci.sh`

## Cross-surface Components

- GUI desktop console docs: `GUI/README.md`
- VS Code addon docs: `vscode-addon/README.md`

Both are aligned with backend RPC surface and governance/health workflows.

## Common Runtime RPC Groups

Representative groups currently exposed:

- Core/runtime: `initialize`, `shutdown`, `runtime.health`, `runtime.stability`, `config.reload`
- Safety/governance: `governance.status`, `governance.plan.get`, `governance.plan.update`, `governance.audit.recent`, `security.baseline`
- Observability: `metrics.get`, `metrics.prometheus`, `trace.get`, `trace.metrics`, `observability.alerts`, `health.probes`
- Reliability: `breaker.status`, `breaker.reset`, `breaker.recovery`, `maintenance.gc`
- Workflow/task: `workflow.execute`, `task.plan`, `task.execute`
- Learning/intelligence: `learning.summary`, `learning.replay`, `learning.guardrail`, `selector.status`, `knowledge.distill`, `rl.alignment.offline_eval`, `hardness.status`
- Optimization/ops: `cost.status`, `config.baseline`, `error.contract`, `build.repro`, `data.lifecycle`, `harness.status`, `optimization.peak`, `quality.baseline`

## Related Docs

- `blue15.md` (implementation and progress ledger)
- `README-PUA-UNIVERSAL.md`
- `MCP_LAYER.md`
- `GO-ON_PUA_IMPLEMENTATION.md`

## License

This project is licensed under MIT or BSD (your choice).
