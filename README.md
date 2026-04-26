# go-on

English | [简体中文](README.zh-CN.md)

go-on is a Rust runtime for ACP/MCP-oriented agent orchestration, governance, and production-safe operations.

## Version

- Core runtime: 0.7.1
- GUI desktop: 0.7.1
- VS Code addon: 0.7.1
- Default feature: `local-acp-sqlite`
- Optional feature scaffold: `server-mcp-postgres`

## Repository Layout

### Core Directories
- `src/`: Backend runtime implementation (Rust)
- `GUI/`: Tauri + Vue desktop console
- `vscode-addon/`: VS Code extension

### Configuration & Scripts
- `config/`: Configuration files (`config.toml`, `config.production.toml`, `providers.toml`)
- `scripts/`: Quality/release gate scripts and deployment utilities
  - `scripts/deploy/nginx/`: Ingress and TLS reverse-proxy templates

### Documentation
- `docs/`: Comprehensive project documentation
  - `docs/blueprints/`: Blueprint documents (blue1.md to blue34.md)
  - `docs/design/`: Design documents and future planning
  - `docs/guides/`: Implementation guides and status documents
  - `docs/reports/`: Project evaluation and code review reports
- `DOC/`: Project documentation in book format

### Testing & Development
- `tests/`: Integration tests and test artifacts
  - `tests/artifacts/`: Test artifacts and benchmark results
  - `tests/requests/`: NDJSON scenario benchmarks and replay inputs
- `test_i18n/`: Internationalization test suites

### Resources & Rules
- `languages/`: Runtime i18n resources and PUA rules
- `RULES/`: Governance and coding rule packs
- `contracts/`: Editor capability matrix and contracts

### Archive & Temporary Files
- `archive/`: Archived temporary files and logs
  - `archive/temp/`: Temporary compilation outputs and logs
  - `archive/logs/`: Runtime log files

### Backend Source Modules (`src/`)
- `acp`: ACP server, request routing, workflow/task/chat handling
- `agents`: Provider adapters and agent contracts
- `core`: Config, setup, readiness, errors
- `governance`: Policy/rule controls and audit support
- `intelligence`: Selectors, reinforcement, quality models
- `optimization`: Cost/speed/reliability/failure-prevention
- `orchestration`: Flow/mode/router/tool orchestration
- `observability`: Metrics/trace/performance helpers
- `memory`: Cache/vector stores
- `protocol`: Protocol server and JSON-RPC support
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

Interpretation:

- `adaptive` means dual-stack protocol capability with request-type aware routing.
- Explicit fixed modes stay config-driven and are never silently overwritten.
- The current startup transport is derived from runtime prerequisites: HTTP when `acp_http_bind_addr` is available, otherwise stdio.

## Quick Start

### 1) Build and Test

```bash
cargo build
cargo check --all-targets
cargo test --all-targets
```

### 2) First-time Setup

```bash
cargo run -- --init --config config/config.toml
cargo run -- --check --config config/config.toml
```

Optional setup levels:

```bash
cargo run -- --init --setup-level quick --config config/config.toml
cargo run -- --init --setup-level standard --config config/config.toml
cargo run -- --init --setup-level custom --config config/config.toml
```

### 3) Start Runtime

- Linux/macOS: `./scripts/start-go-on.sh`
- Windows: `scripts/start-go-on.bat`

Default health endpoint:

- `http://127.0.0.1:8090/health`

## Production Baseline

Production-oriented template:

- `config/config.production.toml`

Current baseline includes:

- Loopback bind by default
- Entry auth + entry rate limiting options
- Strict production fail-fast (`runtime.production_strict = true`)
- OTEL-related runtime settings

### API Key Setup (production mode)

When running with `config/config.production.toml`, entry authentication is enabled by default.
Set the following environment variable **before** starting the server:

```bash
# Linux / macOS
export GO_ON_ENTRY_API_KEY="your-secret-key-here"
./scripts/start-go-on.sh
```

```powershell
# Windows
$env:GO_ON_ENTRY_API_KEY = "your-secret-key-here"
scripts\start-go-on.bat
```

All RPC requests must include the key in the `Authorization` header:

```
Authorization: Bearer your-secret-key-here
```

If this variable is missing or empty the server will reject all requests with error code `-32003` (`AuthRequired`).

> **Security**: Never commit secret values to version control. Use environment variables, a secrets manager, or a keyring-backed injector.

Ingress and TLS templates:

- `scripts/deploy/nginx/go-on.conf`
- `scripts/deploy/nginx/README.md`

Release readiness checklist:

- `docs/RELEASE_READINESS.md`

## Scenario and Gate Tooling

Scenario replay assets are in `tests/requests/` (runtime health, governance, cost, harness, security, release drill, etc.).

Gate scripts (located in `scripts/`):

- `scripts/run-quality-gate.sh`
- `scripts/run-quality-gate.ps1`
- `scripts/run-release-readiness-gate.sh`
- `scripts/run-release-readiness-gate.ps1`
- `scripts/test_ci.sh`

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

## Related Documentation

Comprehensive documentation is organized in the `docs/` directory:

### Blueprints (`docs/blueprints/`)
- `blue1.md` to `blue34.md` - Implementation blueprints and progress ledger

### Design Documents (`docs/design/`)
- `design.md` - System design overview
- `FUTURE*.md` - Future planning documents
- `future-last.md` - Comprehensive future improvement plan

### Guides (`docs/guides/`)
- `README-PUA-UNIVERSAL.md` - PUA universal implementation guide
- `MCP_LAYER.md` - MCP layer implementation details
- `GO-ON_PUA_IMPLEMENTATION.md` - PUA implementation specifics
- `IMPLEMENTATION_STATUS.md` - Current implementation status
- `MIGRATION_STATUS.md` - Migration status and plans
- `PHASE_10_COMPLETE_IMPLEMENTATION.md` - Phase 10 implementation details

### Reports (`docs/reports/`)
- `PROJECT_EVALUATION_REPORT.md` - Comprehensive project evaluation
- `CODE_REVIEW_FINAL_REPORT.md` - Code review findings
- `PHASE_10_DELIVERY_REPORT.md` - Phase 10 delivery report
- `MIGRATION_FINAL_SUMMARY.md` - Migration summary

### Other Key Documents
- `docs/RELEASE_READINESS.md` - Release readiness checklist
- `docs/RULES.md` - Project rules and guidelines
- `docs/DEVELOPMENT_RULES.md` - Development rules and standards

## License

This project is licensed under MIT or BSD (your choice).
