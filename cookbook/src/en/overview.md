# Architecture Overview

`go-on` is a three-surface runtime around a Rust backend:

- **Backend**: the executable owns config loading, provider selection, routing, setup, health checks, protocol negotiation, HTTP or stdio transport, and a 14-bus capability architecture with cognitive modules.
- **GUI**: the EGUI (Rust native) desktop app manages backend discovery, process lifecycle, integration probes, monitoring, chat, and configuration management.
- **VS Code addon**: the extension launches or probes the runtime, exposes RPC-backed commands, and can override protocol mode per workspace.

## Version

- Core runtime: **1.1.0**
- GUI desktop: **1.1.0**
- VS Code addon: **1.1.0**

## GUI Desktop App

The EGUI-based desktop GUI (`gui/`) provides monitoring, chat, and configuration management:

```bash
cargo run --manifest-path gui/Cargo.toml
```

Key features:
- **Monitor**: Backend health, AI provider status, real-time metrics
- **Chat**: Multi-session conversations with phase (coding/review/debug/test/deploy) and mode (Ask/Plan/Edit/Safeguard/Full Auto) selectors, file attachments, dynamic send button based on AI status
- **Skills**: Create and import AI skills; built-in `skill-creator` lets AI define new skills autonomously
- **Settings**: Feature toggles, language switching (en/zh-CN/zh-TW), 5 visual themes
- **Backend Connection**: ACP+HTTP JSON-RPC with automatic health polling

## Build Profiles

Three build profiles support different deployment scenarios, plus a `full` for CI:

| Profile | Backend | Use Case | Build Command |
|:--------|:--------|:---------|:--------------|
| `local` | SQLite + sqlite-vec | Single-user local tool | `cargo build` (default) |
| `simple-server` | SQLite + sqlite-vec | Single-server deployment | `cargo build --no-default-features -F simple-server` |
| `multi-users-server` | PostgreSQL + pgvector | Multi-user production | `cargo build --no-default-features -F multi-users-server` |
| `full` | SQLite (all features) | CI / development | `cargo build --no-default-features -F full` |

## Verification Status

| Profile | `cargo clippy -D warnings` | Tests |
|:--------|:--------------------------:|:-----:|
| **local** | ✅ **Zero warnings** | **2252** |
| **simple-server** | ✅ **Zero warnings** | **all pass** |
| **full** | ✅ **Zero warnings** | **all pass** |
| **multi-users-server** | ✅ **Zero warnings** | **all pass** |

All 2252 unit tests pass with zero failures and zero ignored tests. E2e tests (requiring infrastructure) are marked `#[ignore]` for local runs.

## Runtime Protocol Modes

The backend supports five access modes:

- `adaptive` (recommended default): keep dual-stack capability and route requests by client type while deriving startup transport from runtime prerequisites.
- `acp_stdio`: run ACP over stdio for editor-launched child-process integrations.
- `acp_http`: expose ACP-style HTTP endpoints from a long-running backend process.
- `mcp_stdio`: expose MCP over stdio.
- `mcp_http`: expose MCP and OpenAI-compatible HTTP endpoints.

In this model, explicit fixed modes are still config-driven. `adaptive` is not a silent rewrite to one fixed interface; today it selects an HTTP entry when `--acp-http-bind` is present and otherwise keeps a stdio entry while preserving ACP/MCP request dispatch compatibility.

The HTTP runtime exposes a practical integration surface around `http://127.0.0.1:8090` by default when started with `--acp-http-bind`:

- `/health`
- `/chat`
- `/chat/stream`
- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

That split matters for the three clients:

- Zed external agent flows can use ACP over stdio or ACP over HTTP.
- Zed model-provider style flows can use the OpenAI-compatible `/v1` endpoints.
- The VS Code addon can either use runtime RPC over spawned stdio or probe the runtime through HTTP.
- The GUI uses a local backend executable plus a working directory that contains `config.toml`.

## Architecture: Multi-Bus Capability System

go-on implements a **14-bus architecture** centered on `CapabilityBus` and `HarnessBus`.

### Core Buses

| Bus | Module | Description |
|:----|--------|-------------|
| **CapabilityBus** | `src/intelligence/capability_bus/core.rs` | Central intelligence bus; orchestrates sense/decide/evolve lifecycle |
| **HarnessBus** | `src/governance/harness_bus.rs` | Governance entry; policy evaluation, drift/resilience/security checks |

### Sub-Buses (Phase 4)

| Bus | Module | Description |
|:----|--------|-------------|
| **ToolBus** | `capability_bus/tool_bus.rs` | Unified tool/skill invocation, capability matrix, agent-tool matching |
| **ObservabilityBus** | `capability_bus/observability_bus.rs` | Unified observability: latency, error rates, agent health |
| **OptimizationBus** | `capability_bus/optimization_bus.rs` | Cost/speed/reliability recommendation, circuit breaker |
| **MemoryBus** | `capability_bus/memory_bus.rs` | Cascading cache (L1→L2→L3), vector store lookup |
| **ProtocolBus** | `capability_bus/protocol_bus.rs` | Protocol-aware routing, health/latency tracking |
| **OrchestrationBus** | `capability_bus/orchestration_bus.rs` | Flow/mode/router orchestration, mode recommendation |
| **DistributedMemoryBus** | `capability_bus/distributed_memory_bus.rs` | Cross-node memory sharing (feature-gated) |

### Bus Lifecycle

```
sense()   →  Aggregate agent health, available modes, optimization recommendations
decide()  →  Combine mode recommendation with tool-agent matching
evolve()  →  Update Q-tables, record consensus votes, send evolution events
execute_tool() → HarnessBus evaluate() → ToolBus execute() → ObservabilityBus record()
```

## Security Features

| Feature | Description |
|:--------|:------------|
| **mTLS** | Mutual TLS for ACP HTTP listener with cert-pinning and expiry monitoring |
| **Request signing** | Ed25519 or HMAC-SHA256 for JSON-RPC request authentication |
| **Vault integration** | HashiCorp Vault for secret lifecycle management and rotation |
| **System keyring** | macOS Keychain, Linux Secret Service, Windows Credential Manager |
| **Content safety** | Runtime content scanning with configurable policies (SafeGuard mode) |
| **Prompt injection detection** | Runtime scanning for injection patterns with configurable threshold |

## Observability

go-on provides production-grade observability:

| Capability | Details |
|:-----------|:--------|
| **Prometheus `/metrics` endpoint** | 16+ metrics including latency, throughput, cache hit rates |
| **OpenTelemetry tracing** | OTLP export (default endpoint `localhost:4317`), spans for routing, execution, selection |
| **Governance status endpoint** | Real-time p95 latency, DAG metrics, cache stats via `governance.status` JSON-RPC |
| **OTel stdout exporter** | Fallback trace export when no OTLP collector is available |

## Internationalization (i18n)

go-on provides full i18n coverage (~95%) across the Rust backend:

| Language | File | Keys |
|:---------|:-----|:----:|
| English (US) | `languages/en_US.json` | 448+ |
| Chinese (Simplified) | `languages/zh_CN.json` | 448+ |
| Chinese (Traditional) | `languages/zh_TW.json` | 448+ |

Covered layers: ACP/MCP HTTP errors (100%), agent provider modules (100%, 35 providers), config validation (100%), CLI setup (100%), API handler errors (100%), orchestration (100%), GUI (~98%), VS Code addon (70+ keys).

## Repository areas that map to the architecture

- `src/`: backend runtime, CLI, setup, ACP and MCP implementation.
  - `src/acp/`: ACP server, request routing, workflow/task/chat/checkpoint
  - `src/agents/`: Provider adapters (OpenAI, Anthropic, DeepSeek, Gemini, xAI Grok, SiliconFlow, and 30+ more), AgentFactory
  - `src/core/`: Config, setup, readiness, error model
  - `src/governance/`: Policy/rule governance, audit, security governor, drift protection
  - `src/intelligence/`: Selectors, RL, capability bus, discovery, consensus, evolution
  - `src/orchestration/`: Flow/mode/router, brain loop, omnipotent mode, artifact
  - `src/fault_tolerance.rs`: Cross-node fault tolerance engine
  - `src/resilience/`: Hyper resilience engine
  - `src/protocol/`: Protocol server, JSON-RPC, multi-channel transport
  - `src/i18n/`: Language runtime
- `gui/`: EGUI (Rust native) desktop GUI
- `vscode-addon/`: VS Code extension with i18n (en_US, zh_CN, zh_TW)
- `config/`: Configuration files
- `tests/`: Integration tests and replay assets
- `scripts/`: Quality/release gate scripts

## Recommended operator flow

For a new machine or a new workspace, the shortest path is:

1. Build or obtain the `go-on` backend executable.
2. Run `go-on --setup --setup-level standard`.
3. Verify readiness with `go-on --status`.
4. If an HTTP client is involved, start the backend with `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090`.
5. Attach Zed, the VS Code addon, or the GUI depending on your front end.

The next chapters expand each part in detail.