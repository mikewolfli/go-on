<p align="center">
  <img src="snapshots/head.png" alt="go-on" width="600">
</p>

<p align="center">
  <strong>go-on</strong> — A Rust-based ACP/MCP agent orchestration runtime with desktop GUI, VS Code extension, and multi-AI-provider support.
</p>

<p align="center">
  English | <a href="README.zh-CN.md">简体中文</a>
</p>

---

[![Rust](https://img.shields.io/badge/rust-1.1.0-orange?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-2252-brightgreen)]()
[![Providers](https://img.shields.io/badge/providers-35+-9cf)]()
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()
[![LOC](https://img.shields.io/badge/code-265K-blue)]()

## What is go-on?

go-on is a **local-first**, production-grade AI agent orchestration runtime written in Rust. It bridges large language models with your tools and workflows through standard agent protocols (ACP / MCP). You can run it as a CLI, a desktop GUI app, or a backend server — with autonomous loops, tool orchestration, and built-in governance.

**Use go-on to:**
- 🖥️ Chat with AI models via a native desktop GUI or terminal
- 🤖 Run autonomous agents that plan, execute, and self-correct
- 🔧 Orchestrate multi-tool workflows with dependency-aware DAG execution
- 🔌 Connect AI models to MCP servers or act as an MCP server yourself
- 🛡️ Enforce governance policies with RBAC, audit trails, and risk assessment
- 🧩 Extend via VS Code extension or Rust/Python SDK

## Quick Start

```bash
# Build and run (creates default config automatically)
cargo run

# Open the desktop GUI
cargo run --manifest-path gui/Cargo.toml

# Terminal chat mode
cargo run -- --chat

# Start as MCP server
cargo run -- --protocol-mode mcp_stdio
```

First run opens an interactive setup wizard if no AI providers are configured.

**Full documentation**: see the `cookbook/` directory (mdBook format with trilingual support) — `cd cookbook && mdbook serve --open`

Default health endpoint: `http://127.0.0.1:8090/health`

---

## Screenshots

| Monitor Dashboard | Chat Interface |
|:---:|:---:|
| ![Monitor](snapshots/monitor.png) | ![Chat](snapshots/chat.png) |

| Provider Management | Settings |
|:---:|:---:|
| ![Providers](snapshots/providers.png) | ![Settings](snapshots/settings.png) |

| Skills & Tools |
|:---:|
| ![Skills](snapshots/skills.png) |

---

## Features

### Agent Orchestration
- **Autonomous agent loop** — Plan → Execute → Reflect → Replan, with complexity-adaptive iteration
- **DAG task execution** — Kahn topological sort, dependency edges, parallel group execution, cycle detection
- **Full-auto flow** — Parse intent → discover skills → prepare environment → execute → report
- **Fast path cache** — SHA-256 fingerprint, TTL/LRU eviction, 4-tier caching (intent/skill/env/route)
- **Multi-model voter** — Concurrent agent voting for high-stakes decisions (majority/weighted/unanimous/fusion)

### AI Provider Support (35+)
OpenAI · Anthropic · DeepSeek · Gemini · xAI Grok · Groq · Mistral · Qwen · Llama · SiliconFlow · Cohere · AI21 · Perplexity · Together · Fireworks · Replicate · MiniMax · Moonshot · Zhipu GLM · Baidu Qianfan · ByteDance Doubao · Tencent Hunyuan · StepFun · Skywork · Yi · Kimi · NIM · Aleph Alpha · DeepQuest · FaceWall · LoopAI · Langboat · Titan · Wenxin · Xihu

Native function calling is supported for OpenAI, Anthropic, DeepSeek, Gemini, Groq, and xAI Grok.

### Protocols & Transport
- **ACP** (Agent Client Protocol) — stdio + HTTP, JSON-RPC 2.0
- **MCP** (Model Context Protocol) — stdio + HTTP, tool list/call, streaming, cancellation, timeout
- **5 modes**: `adaptive` (dual-stack), `acp-stdio`, `acp-http`, `mcp-stdio`, `mcp-http`
- **Cross-entry parity** — consistent stop_reason and round count across ACP/CLI/MCP

### Tool System
- **16+ built-in tools** — read/write/search/apply_patch/run_tests/inspect_git_diff/shell_exec/http_request/db_query/grep/find/git/cargo/npm/docker/pip
- **Tool pipeline** — serial/parallel/conditional execution with error handling
- **Tool transactions** — idempotency keys, WAL persistence, compensation actions, two-phase commit (2PC)
- **Dynamic tool recommendation** — pattern + recency + co-occurrence based suggestions
- **Native function calling** — OpenAI/Anthropic tool_choice, Gemini functionCall, DeepSeek tools

### Governance & Safety
- **HarnessBus** — central governance with policy evaluation, drift detection, security checks
- **PUA rules engine** — real-time policy evaluation with escalation levels
- **RBAC** — role-based access control with tenant registration
- **Tenant isolation** — cross-tenant blocking; budget-aware concurrency limits
- **Audit trail** — full decision pipeline recording with replay capability
- **Audit integrity** — hash-chain verified audit entries for tamper detection
- **Prompt injection detection** — runtime scanning for injection patterns with configurable threshold
- **Content safety checking** — SafeGuard mode for AI-powered risk assessment

### Performance
- **FastPathCache** — sub-millisecond cache lookup for repeated queries
- **SSE buffer pool** — zero-allocation streaming event serialization
- **Cache warming** — predictive pre-warming with adaptive TTL
- **Concurrent execution** — per-role BinaryHeap dequeue (O(log n)), semaphore backpressure
- **DAG Join timeout** — prevents single slow tool from stalling the pipeline

### Resilience
- **Recovery orchestrator** — 6 strategies: Retry → Reroute → Replan → Repair → Escalate → Degrade
- **Chaos testing** — 10 fault injection types (timeout, partition, crash, corruption, rate-limit)
- **HyperResilience** — circuit breaker state machine, failover groups, self-healing
- **Hot failover** — primary-to-fallback model switching with blacklist cooldown

### Observability
- **Prometheus `/metrics` endpoint** — 16+ metrics including latency, throughput, cache hit rates
- **OpenTelemetry tracing** — OTLP/stdout export, spans for routing, execution, selection
- **Governance status endpoint** — real-time p95 latency, DAG metrics, cache stats
- **Audit replay** — full task execution evidence chain, filterable

### Session Management
- **Session context** — key concept extraction, message importance scoring, continuity markers
- **Session compression** — semantic compression of overflow messages
- **Context window budget** — intelligent message retention within token limits

### Security
- **Request signing** — Ed25519 or HMAC-SHA256 for JSON-RPC request authentication
- **mTLS** — mutual TLS for ACP HTTP listener with cert-pinning and expiry monitoring
- **Secret rotation** — HashiCorp Vault integration for key lifecycle management
- **System keyring** — macOS Keychain, Linux Secret Service, Windows Credential Manager
- **Content safety** — runtime content scanning with configurable policies

### Configuration & Deployment
- **Hot-reload config** — file-watch based, atomic swap at runtime
- **Schema versioning** — semver-tracked config with migration
- **4 build profiles** — local (SQLite), simple-server (SQLite), multi-users-server (PostgreSQL + pgvector), full (all features)
- **Docker** — production containers with HEALTHCHECK, k8s manifests available
- **OTel** — distributed tracing via OTLP collector (default: `localhost:4317`)
- **Trilingual i18n** — English, Simplified Chinese, Traditional Chinese (~95% coverage across backend, GUI, VS Code)

---

## Architecture

go-on uses a **14-bus capability architecture** with cognitive modules:

```
┌────────────────────────────────────────────────────────────┐
│                    HarnessBus (Governance)                  │
│  Policy · Drift Detection · Resilience · Security · Audit  │
├────────────────────────────────────────────────────────────┤
│                   CapabilityBus (Intelligence)              │
│  Sense → Decide → Act → Feedback → Evolve                 │
├──────────┬──────────┬──────────┬──────────┬───────────────┤
│ ToolBus  │ ObservB. │ OptimizB.│ MemoryBus│ ProtocolBus   │
├──────────┼──────────┼──────────┼──────────┼───────────────┤
│ OrchestB.│          │          │ DistMemB.│               │
└──────────┴──────────┴──────────┴──────────┴───────────────┘
```

### Key Capability Modules

| Module | Description |
|:-------|:------------|
| **HarnessBus** | Central policy engine: evaluate/validate/verify, PUA rules, RBAC, drift detection, hyper-resilience, audit trail |
| **CapabilityBus** | Multi-factor agent selection (reputation + task-fit + outcome) with causal Bayesian graph for routing |
| **Planner** | Task-adaptive DAG planning with dependency inference |
| **BrainLoop** | Plan → Execute → Reflect → Replan cognitive cycle |
| **DAG Driver** | Topological execution with parallel group scheduling |
| **SelfModelCore** | System self-awareness and capability tracking |
| **MetacognitiveController** | Observation-driven reflection and corrective action |
| **WorldModel** | Entity/event/relationship tracking with causal insight |
| **FederatedRL** | Distributed reinforcement learning across nodes |
| **HyperResilience** | Circuit breaker, failover group, self-healing |
| **MultiChannelTransport** | QoS-aware, prioritized message transport |

---

## Extensions

### VS Code Addon
`vscode-addon/` contains a VS Code extension that launches go-on and exposes 60+ commands — chat, workflow execution, skill management — inside the editor.

```bash
cd vscode-addon
npm install
npm run compile
```

### SDKs
- **Rust SDK** (`sdk/rust/`) — Strongly typed client with methods across multiple domains
- **Python SDK** (`sdk/python/`) — HTTPX-based async client with streaming support
- **Node.js SDK** (`sdk/nodejs/`) — TypeScript async client with 30+ methods across all API domains

---

## Codebase Statistics

| Metric | Value |
|:-------|:------|
| Rust backend LOC | ~226K (443 modules) |
| GUI (EGUI) LOC | ~22K |
| VS Code addon (TypeScript) LOC | ~17K |
| SDK (Rust + Python) LOC | ~1.2K |
| Trilingual i18n | en / zh-CN / zh-TW (~95% coverage) |

## Build Profiles

| Profile | Backend | Use Case |
|:--------|:--------|:---------|
| `local` | SQLite + sqlite-vec | Single-user local tool (default) |
| `simple-server` | SQLite + sqlite-vec | Single-server deployment |
| `multi-users-server` | PostgreSQL + pgvector | Multi-user production |
| `full` | SQLite (all features) | CI / development |

```bash
# Build commands
cargo build                                                    # local (default)
cargo build --no-default-features --features simple-server
cargo build --no-default-features --features multi-users-server,backend-postgres
```

## Verification

| Profile | `cargo clippy -D warnings` | Test Status |
|:--------|:--------------------------:|:-----------:|
| `local` | ✅ **Zero warnings** | ✅ **2252 pass, 0 fail, 0 ignored** |
| `simple-server` | ✅ **Zero warnings** | ✅ **all pass** |
| `multi-users-server` | ✅ **Zero warnings** | ✅ **all pass** |
| `full` | ✅ **Zero warnings** | ✅ **all pass** |

All 4 build profiles compile with zero clippy warnings. Unit tests (2252) all pass with zero failures and zero ignored tests. E2e integration tests require running infrastructure.

---

## License

MIT License — see [LICENSE](LICENSE) for details.
