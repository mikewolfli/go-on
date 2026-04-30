# go-on

English | [简体中文](README.zh-CN.md)

go-on is a Rust runtime for **ACP/MCP-oriented agent orchestration, governance, and production-safe operations**,
with full i18n support and a modular multi-bus architecture spanning 14 capability buses and 21+ F-GAP modules.

## Version

- Core runtime: **0.8.2**
- GUI desktop: **0.8.2**
- VS Code addon: **0.8.2**
- Default feature: `profile-local`
- Alternative feature scaffolds: `profile-simple-server`, `profile-multi-users-server`

## Build Profiles

Three build profiles support different deployment scenarios:

| Profile | Backend | Target | Build Command |
|---------|---------|--------|---------------|
| `profile-local` | SQLite + sqlite-vec | Single-user local tool | `cargo build` (default) |
| `profile-simple-server` | SQLite + sqlite-vec | Single-server deployment | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | Multi-user production | `cargo build --no-default-features -F profile-multi-users-server` |

## Verification Status (Phase 4 — Complete)

| Profile | `cargo check` | `cargo clippy -D warnings` | `cargo test` |
|---------|:-----------:|:------------------------:|:----------:|
| **profile-local** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **866 passed** (766 unit + 86 RPC + 14 transport) |
| **profile-simple-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **905 passed** |
| **profile-multi-users-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **898 passed** |

## Repository Layout

### Core Directories
- `src/` — Backend runtime implementation (Rust)
  - `src/acp/` — ACP server, request routing, workflow/task/chat/checkpoint handling
  - `src/agents/` — Provider adapters (OpenAI, Anthropic, DeepSeek, Ollama, etc.) and agent contracts
    - `src/agents/factory/` — AgentFactory with feature-gated profile selection
  - `src/core/` — Config, setup, readiness, error model
  - `src/governance/` — Policy/rule governance, audit, security governor, drift protection
    - `src/governance/drift/` — Drift protection engine (F-GAP-26)
    - `src/governance/harness_bus.rs` — HarnessBus governance entry
  - `src/intelligence/` — Selectors, RL, quality models, capability bus, discovery, consensus
    - `src/intelligence/capability_bus/` — **14 sub-buses** (core + tool + observability + optimization + memory + protocol + orchestration + distributed_memory)
    - `src/intelligence/discovery.rs` — DiscoveryCenter (F-GAP-11)
    - `src/intelligence/matcher.rs` — ScenarioMatcher (F-GAP-12)
    - `src/intelligence/evolution_graph.rs` — EvolutionGraph (F-GAP-18)
    - `src/intelligence/metacognitive.rs` — MetacognitiveController (F-GAP-22)
    - `src/intelligence/self_model.rs` — SelfModelCore (F-GAP-21)
    - `src/intelligence/consensus.rs` — ConsensusEngine (F-GAP-16)
    - `src/intelligence/consciousness.rs` — ConsciousnessMetrics (F-GAP-25)
    - `src/intelligence/world_model.rs` — WorldModel pipeline (F-GAP-23)
    - `src/intelligence/continuous_learning.rs` — ContinuousLearningCenter (F-GAP-24)
  - `src/orchestration/` — Flow/mode/router/orchestration, brain loop, omnipotent mode
    - `src/orchestration/loop/brain_loop.rs` — Brain Loop engine (F-GAP-17)
    - `src/orchestration/council/` — OrchestrationCouncil (F-GAP-15)
    - `src/orchestration/omnipotent.rs` — OmnipotentMode (F-GAP-09)
    - `src/orchestration/artifact.rs` — ArtifactLayer (F-GAP-10)
    - `src/orchestration/skill_import.rs` — RemoteSkill (F-GAP-10)
    - `src/orchestration/scheduler.rs` — TaskScheduler (ARCH-02)
    - `src/orchestration/task_graph_store.rs` — TaskGraphStore (F-GAP-03)
  - `src/fault_tolerance.rs` — FaultToleranceEngine (F-GAP-28), cross-node fault isolation & auto-recovery
  - `src/resilience/` — HyperResilienceEngine (F-GAP-27)
    - `src/resilience/hyper_resilience.rs` — Circuit breaker, failover, self-healing
  - `src/i18n/` — Language runtime (~95% i18n coverage across all backend modules)
  - `src/mcp/` — MCP adapter helpers
  - `src/memory/` — Cache and vector store abstractions
  - `src/observability/` — Metrics/trace/performance helpers
  - `src/optimization/` — Cost/speed/reliability optimization
  - `src/protocol/` — Protocol server, JSON-RPC support, multi-channel transport
  - `src/shared/` — Shared types, protocol mode, tool descriptors
- `GUI/` — Tauri + Vue desktop console
- `vscode-addon/` — VS Code extension with i18n (en_US, zh_CN, zh_TW)

### Configuration & Scripts
- `config/` — Configuration files (`config.toml`, `config.production.toml`, `providers.toml`)
- `scripts/` — Quality/release gate scripts and deployment utilities
  - `scripts/deploy/nginx/` — Ingress and TLS reverse-proxy templates

### Documentation
- `docs/` — Comprehensive project documentation
  - `docs/blueprints/` — Blueprint documents (blue1.md to blue38.md and FAULT1)
  - `docs/design/` — Design documents (FUTURE1-6, future-last)
  - `docs/guides/` — Implementation guides (MCP, PUA, migration, GUI, model selection)
  - `docs/reports/` — Evaluation and code review reports
- `DOC/` — Project documentation in book format

### Testing & Development
- `tests/` — Integration tests and test artifacts
  - `tests/artifacts/` — Test artifacts and benchmark results
  - `tests/requests/` — NDJSON scenario benchmarks and replay inputs
  - Integration tests: ACP RPC, protocol consistency, transport parity, OpenAI compat matrix, PUA contract smoke
- `test_i18n/` — Internationalization test suites

### Resources
- `languages/` — Runtime i18n resources (en_US, zh_CN, zh_TW — 372+ keys each)
  - `languages/rules/` — PUA coding rules
- `RULES/` — Governance and coding rule packs
- `contracts/` — Editor capability matrix and contracts

## Runtime Protocol Modes

`[protocol].mode` supports 5 values:

- `adaptive` (recommended default) — dual-stack protocol, request-type aware routing
- `acp_stdio` — ACP over stdio
- `acp_http` — ACP over HTTP
- `mcp_stdio` — MCP over stdio
- `mcp_http` — MCP over HTTP

Example:

```toml
[protocol]
mode = "adaptive"
```

## Architecture: Multi-Bus Capability System

go-on implements a **14-bus architecture** centered on `CapabilityBus` and `HarnessBus`:

### Core Buses (Phase 0-3)
| Bus | Module | Description |
|:----|--------|-------------|
| **CapabilityBus** | `capability_bus/core.rs` | Central intelligence bus; orchestrates sense/decide/evolve lifecycle |
| **HarnessBus** | `governance/harness_bus.rs` | Governance entry; policy evaluation, drift/resilience/security checks |

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

### F-GAP Modules (Phase 4 — 21/21 Complete ✅)

| F-GAP | Module | Location | Status |
|:-----:|--------|----------|:------:|
| 09 | OmnipotentMode | `orchestration/omnipotent.rs` | ✅ 20 tests |
| 10 | ArtifactLayer + RemoteSkill | `orchestration/artifact.rs`, `orchestration/skill_import.rs` | ✅ 13 tests |
| 11 | DiscoveryCenter | `intelligence/discovery.rs` | ✅ 11 tests |
| 12 | ScenarioMatcher | `intelligence/matcher.rs` | ✅ 9 tests |
| 13 | AgentFactory | `agents/factory/` | ✅ Feature-gated |
| 14 | SecurityGovernor | `governance/security_governor.rs` | ✅ |
| 15 | OrchestrationCouncil | `orchestration/council/` | ✅ 22 tests |
| 16 | ConsensusEngine | `intelligence/consensus.rs` | ✅ 20 tests |
| 17 | BrainLoop | `orchestration/loop/brain_loop.rs` | ✅ 32 tests |
| 18 | EvolutionGraph | `intelligence/evolution_graph.rs` | ✅ 12 tests |
| 19 | FederatedRL | `intelligence/reinforcement/federated.rs` | ✅ 27 tests |
| 20 | DistributedMemory (network) | `capability_bus/distributed_memory_bus.rs` | ✅ Enhanced |
| 21 | SelfModelCore | `intelligence/self_model.rs` | ✅ 12 tests |
| 22 | MetacognitiveController | `intelligence/metacognitive.rs` | ✅ 12 tests |
| 23 | WorldModel | `intelligence/world_model.rs` | ✅ |
| 24 | ContinuousLearning | `intelligence/continuous_learning.rs` | ✅ |
| 25 | ConsciousnessMetrics | `intelligence/consciousness.rs` | ✅ 12 tests |
| 26 | DriftProtection | `governance/drift/drift_protection.rs` | ✅ 12 tests |
| 27 | HyperResilience | `resilience/hyper_resilience.rs` | ✅ |
| 28 | FaultTolerance | `fault_tolerance.rs` | ✅ 20 tests (incl. E2E, 500-node stress) |
| 29 | MultiChannelTransport | `protocol/transport.rs` | ✅ 37 tests (QoS, Dedup, Peek) |

### 38-Dimensional Full Star Rating

```
Governance & Compliance (5/5):    ★★★★★ ProvenanceLedger, DriftProtection, PolicyEvaluator, TokenLayerChain, SecurityGovernor
Resilience & Fault Tolerance (2/2):★★★★★ HyperResilienceEngine, FaultToleranceEngine
Orchestration & Execution (6/6):  ★★★★★ OrchestrationBus, TaskScheduler, ExecutionGraph, OmnipotentMode, ArtifactLayer, BrainLoop
Routing & Scheduling (7/7):       ★★★★★ CapabilityGraph, ReputationStore, QLearningAgent, ScenarioMatcher, DiscoveryCenter, WorkflowRegistry, AgentFactory
Protocol & Transport (2/2):       ★★★★★ ProtocolBus, MultiChannelTransport
Memory & Cache (2/2):             ★★★★★ MemoryBus, DistributedMemoryBus
Observability & Optimization (3/3):★★★★★ ObservabilityBus, OptimizationBus, ToolBus
Intelligent Cognition (5/5):      ★★★★★ Deep Knowledge Distillation, Deep RL, Skill Retention, AI Evolution, Self-built Skills
Self-Cognition (5/5):             ★★★★★ SelfModelCore, ConsciousnessMetrics, MetacognitiveController, WorldModel, ConsensusEngine
───────────────────────────────────────────────────────────────────────────────────
Total (38/38):                    100% ★★★★★
```

### Overall Completion Rate

```
Phase 0: Core Dual Buses         ████████████████████ 100%
Phase 1: Sub-Bus Integration     ████████████████████ 100%
Phase 2: Remaining Fixes         ████████████████████ 100%
Phase 3: ARCH Extension Points   ████████████████████ 100%
Phase 4: FutureDesign (F-GAP)    ████████████████████ 100% (21/21)
Phase 5: Production Hardening    ████████████████████ 100%
────────────────────────────────────────────────────────
Overall:                         ████████████████████ 100%
```

## Internationalization (i18n)

go-on provides full i18n coverage (~95%) across the Rust backend:

| Language | File | Keys |
|:---------|:-----|:----:|
| English (US) | `languages/en_US.json` | 372+ |
| Chinese (Simplified) | `languages/zh_CN.json` | 372+ |
| Chinese (Traditional) | `languages/zh_TW.json` | 372+ |

Covered layers:
- **ACP/MCP HTTP error responses** — 100%
- **Agent provider modules** (OpenAI, Anthropic, DeepSeek, Ollama, etc.) — 100%
- **Config validation** (~50 strings) — 100%
- **CLI setup messages** — 100%
- **API handler errors** — 100%
- **Orchestration** (tool, skill, brain loop) — 100%
- **GUI (Vue/TypeScript)** — ~98%
- **VS Code addon** — 70+ MessageKeys in 3 languages

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

- **Core/runtime**: `initialize`, `shutdown`, `runtime.health`, `runtime.stability`, `config.reload`
- **Safety/governance**: `governance.status`, `governance.plan.get`, `governance.plan.update`, `governance.audit.recent`, `security.baseline`
- **Observability**: `metrics.get`, `metrics.prometheus`, `trace.get`, `trace.metrics`, `observability.alerts`, `health.probes`
- **Reliability**: `breaker.status`, `breaker.reset`, `breaker.recovery`, `maintenance.gc`
- **Workflow/task**: `workflow.execute`, `task.plan`, `task.execute`
- **Learning/intelligence**: `learning.summary`, `learning.replay`, `learning.guardrail`, `selector.status`, `knowledge.distill`, `rl.alignment.offline_eval`, `hardness.status`
- **Optimization/ops**: `cost.status`, `config.baseline`, `error.contract`, `build.repro`, `data.lifecycle`, `harness.status`, `optimization.peak`, `quality.baseline`

## Related Documentation

Comprehensive documentation is organized in the `docs/` directory:

### Blueprints (`docs/blueprints/`)
- `blue1.md` to `blue38.md` — Implementation blueprints and progress ledger
- `FAULT1.MD` — Fault tolerance blueprint
- `server-blue1.md` — Server architecture blueprint

### Design Documents (`docs/design/`)
- `design.md` — System design overview
- `FUTURE.md` to `FUTURE6.md` — Future planning documents
- `future-last.md` — Comprehensive future improvement plan

### Guides (`docs/guides/`)
- `README-PUA-UNIVERSAL.md` — PUA universal implementation guide
- `MCP_LAYER.md` — MCP layer implementation details
- `GO-ON_PUA_IMPLEMENTATION.md` — PUA implementation specifics
- `IMPLEMENTATION_STATUS.md` — Current implementation status
- `MIGRATION_STATUS.md` — Migration status and plans
- `PHASE_10_COMPLETE_IMPLEMENTATION.md` — Phase 10 implementation details
- `ENHANCEMENT_OPPORTUNITIES.md` — Enhancement opportunities (CN/EN)
- `MODEL_SELECTION.md` — Model selection guide
- `GUI_FIRST_RUN.md` — GUI first run guide

### Reports (`docs/reports/`)
- `PROJECT_EVALUATION_REPORT.md` — Comprehensive project evaluation
- `CODE_REVIEW_FINAL_REPORT.md` — Code review findings
- `PHASE_10_DELIVERY_REPORT.md` — Phase 10 delivery report
- `MIGRATION_FINAL_SUMMARY.md` — Migration summary

### Other Key Documents
- `docs/RELEASE_READINESS.md` — Release readiness checklist
- `docs/RULES.md` — Project rules and guidelines
- `docs/DEVELOPMENT_RULES.md` — Development rules and standards
- `docs/CLAUDE.md` — Claude.ai integration guide
- `docs/SAFEGUARD_MODE.md` — Safeguard mode documentation

## License

This project is licensed under MIT or BSD (your choice).
