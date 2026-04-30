# Architecture Overview

`go-on` is a three-surface runtime around a Rust backend:

- **Backend**: the executable owns config loading, provider selection, routing, setup, health checks, protocol negotiation, HTTP or stdio transport, and a 14-bus capability architecture with 21 F-GAP modules.
- **GUI**: the Tauri desktop console manages backend discovery, process lifecycle, integration probes, and local operator workflows.
- **VS Code addon**: the extension launches or probes the runtime, exposes RPC-backed commands, and can override protocol mode per workspace.

## Version

- Core runtime: **0.8.3**
- GUI desktop: **0.8.3**
- VS Code addon: **0.8.3**

## Build Profiles

Three build profiles support different deployment scenarios:

| Profile | Backend | Target | Build Command |
|---------|---------|--------|---------------|
| `profile-local` | SQLite + sqlite-vec | Single-user local tool | `cargo build` (default) |
| `profile-simple-server` | SQLite + sqlite-vec | Single-server deployment | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | Multi-user production | `cargo build --no-default-features -F profile-multi-users-server` |

## Verification Status (Phase 4 Complete)

| Profile | `cargo check` | `cargo clippy -D warnings` | `cargo test` |
|---------|:-----------:|:------------------------:|:----------:|
| **profile-local** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **866 passed** (766 unit + 86 RPC + 14 transport) |
| **profile-simple-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **905 passed** |
| **profile-multi-users-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **898 passed** |

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

## F-GAP Modules (Phase 4 — 21/21 Complete ✅)

go-on implements 21 FutureDesign modules across six capability domains:

### Orchestration & Execution (F-GAP-09, 10, 15, 17)
- **OmnipotentMode** (F-GAP-09): Escalation token, RAII session guard, audit logging
- **ArtifactLayer** (F-GAP-10): Artifact schema registration, storage, TTL pruning
- **RemoteSkill** (F-GAP-10): Remote MCP endpoint as Skill trait
- **OrchestrationCouncil** (F-GAP-15): Multi-agent coordination committee
- **BrainLoop** (F-GAP-17): Plan→Execute→Reflect→Replan full cycle

### Intelligence & Learning (F-GAP-11, 12, 16, 18, 19, 21, 22, 23, 24, 25)
- **DiscoveryCenter** (F-GAP-11): Solution pattern registry and search
- **ScenarioMatcher** (F-GAP-12): Multi-dimension scenario matching
- **ConsensusEngine** (F-GAP-16): Distributed voting and consensus
- **EvolutionGraph** (F-GAP-18): 6-stage capability evolution lifecycle
- **FederatedRL** (F-GAP-19): FedAvg/FedWeighted/FedMedian aggregation
- **SelfModelCore** (F-GAP-21): Self-capability assessment and confidence
- **MetacognitiveController** (F-GAP-22): 6-stage thinking trace, stuck detection
- **WorldModel** (F-GAP-23): World model pipeline
- **ContinuousLearningCenter** (F-GAP-24): Continuous learning orchestration
- **ConsciousnessMetrics** (F-GAP-25): 5-dimension awareness metrics

### Governance & Security (F-GAP-14, 26)
- **SecurityGovernor** (F-GAP-14): Security policy governance
- **DriftProtection** (F-GAP-26): 5 drift types, 4 severity levels, trend detection

### Resilience & Fault Tolerance (F-GAP-27, 28)
- **HyperResilienceEngine** (F-GAP-27): Circuit breaker, failover, self-healing
- **FaultToleranceEngine** (F-GAP-28): Node heartbeat, isolation, auto-recovery, cluster health scoring

### Protocol & Transport (F-GAP-29)
- **MultiChannelTransport** (F-GAP-29): 6 channels, 4 priority levels, QoS, dedup, peek

### Agent Infrastructure (F-GAP-13)
- **AgentFactory** (F-GAP-13): Feature-gated agent instantiation

## 38-Dimension Star Rating

```
Governance & Compliance (5/5):    ★★★★★ ProvenanceLedger, DriftProtection, PolicyEvaluator, TokenLayerChain, SecurityGovernor
Resilience & Fault Tolerance (2/2):★★★★★ HyperResilienceEngine, FaultToleranceEngine
Orchestration & Execution (6/6):  ★★★★★ OrchestrationBus, TaskScheduler, ExecutionGraph, OmnipotentMode, ArtifactLayer, BrainLoop
Routing & Scheduling (7/7):       ★★★★★ CapabilityGraph, ReputationStore, QLearningAgent, ScenarioMatcher, DiscoveryCenter, WorkflowRegistry, AgentFactory
Protocol & Transport (2/2):       ★★★★★ ProtocolBus, MultiChannelTransport
Memory & Cache (2/2):             ★★★★★ MemoryBus, DistributedMemoryBus
Observability & Optimization (3/3):★★★★★ ObservabilityBus, OptimizationBus, ToolBus
Intelligent Cognition (5/5):      ★★★★★ Knowledge Distillation, Deep RL, Skill Retention, AI Evolution, Self-built Skills
Self-Cognition (5/5):             ★★★★★ SelfModelCore, ConsciousnessMetrics, MetacognitiveController, WorldModel, ConsensusEngine
Total (38/38):                    100% ★★★★★
```

## Overall Completion Rate

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

Covered layers: ACP/MCP HTTP errors (100%), agent provider modules (100%), config validation (100%), CLI setup (100%), API handler errors (100%), orchestration (100%), GUI (~98%), VS Code addon (70+ keys).

## Repository areas that map to the architecture

- `src/`: backend runtime, CLI, setup, ACP and MCP implementation.
  - `src/acp/`: ACP server, request routing, workflow/task/chat/checkpoint
  - `src/agents/`: Provider adapters (OpenAI, Anthropic, DeepSeek, Ollama), AgentFactory
  - `src/core/`: Config, setup, readiness, error model
  - `src/governance/`: Policy/rule governance, audit, security governor, drift protection
  - `src/intelligence/`: Selectors, RL, capability bus, discovery, consensus, evolution
  - `src/orchestration/`: Flow/mode/router, brain loop, omnipotent mode, artifact
  - `src/fault_tolerance.rs`: Cross-node fault tolerance engine
  - `src/resilience/`: Hyper resilience engine
  - `src/protocol/`: Protocol server, JSON-RPC, multi-channel transport
  - `src/i18n/`: Language runtime
- `GUI/`: Tauri desktop console
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