<p align="center">
  <img src="snapshots/chat.png" alt="go-on" width="600">
</p>

<p align="center">
  <strong>go-on</strong> — A Rust-based ACP/MCP agent orchestration runtime with desktop GUI, VS Code extension, and 35+ AI provider support.
</p>

<p align="center">
  English | <a href="README.zh-CN.md">简体中文</a>
</p>

---

[![Rust](https://img.shields.io/badge/rust-1.1.0-orange?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-1400%2B-brightgreen)]()
[![Providers](https://img.shields.io/badge/providers-35%2B-9cf)]()
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()

## What is go-on?

go-on is a **local-first**, production-grade AI agent runtime written in Rust. It bridges large language models with your tools and workflows through standard agent protocols (ACP / MCP). You can run it as a CLI, a desktop app, or a backend server — with full autonomy loop, tool orchestration, and governance built in.

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
cargo run -- --protocol-mode mcp-stdio
```

First run opens an interactive setup wizard if no AI providers are configured.  
Default health check: `http://127.0.0.1:8090/health`

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
- **Predictive reroute** — Proactive agent switching based on health scoring, not just failure recovery

### AI Provider Support (35+)
OpenAI · Anthropic · DeepSeek · Gemini · Groq · Ollama · Mistral · Qwen · Llama · Cohere · AI21 · Perplexity · Together · Fireworks · Replicate · MiniMax · Moonshot · Zhipu GLM · Baidu Qianfan · ByteDance Doubao · Tencent Hunyuan · StepFun · Skywork · and more.

Native function calling is supported for OpenAI, Anthropic, DeepSeek, and Gemini providers.

### Protocols & Transport
- **ACP** (Agent Client Protocol) — stdio + HTTP, with JSON-RPC 2.0
- **MCP** (Model Context Protocol) — stdio + HTTP, tool list/call, streaming, cancellation, timeout
- **5 modes**: `adaptive` (dual-stack), `acp-stdio`, `acp-http`, `mcp-stdio`, `mcp-http`
- **Cross-entry parity** — same task produces consistent stop_reason and round count across ACP/CLI/MCP

### Tool System
- **16 built-in tools** — read/write/search/apply_patch/run_tests/inspect_git_diff/shell_exec/http_request/db_query/grep/find/git/cargo/npm/docker/pip
- **Tool pipeline** — serial/parallel/conditional execution with configurable error handling
- **Tool transactions** — idempotency keys, WAL persistence, compensation actions, two-phase commit (2PC)
- **Dynamic tool recommendation** — pattern + recency + co-occurrence based suggestions
- **Native function calling** — OpenAI/Anthropic tool_choice, Gemini functionCall, DeepSeek tools parameter

### Governance & Safety
- **HarnessBus** — central governance layer with policy evaluation, drift detection, and security checks
- **PUA rules engine** — real-time policy evaluation with escalation levels
- **RBAC** — role-based access control with multi-source tenant registration
- **Tenant isolation** — cross-tenant access blocked; budget-aware concurrency limits
- **Audit trail** — full decision pipeline recording with replay capability
- **Safeguard mode** — AI-powered risk assessment for high-stakes operations

### Performance
- **FastPathCache** — sub-millisecond cache lookup for repeated intent/skill/env queries
- **SSE buffer pool** — zero-allocation streaming event serialization
- **Cache warming** — predictive pre-warming with adaptive TTL and multi-tier management
- **Concurrent execution** — per-role BinaryHeap dequeue (O(log n)), semaphore-based backpressure
- **DAG Join timeout** — tokio::time::timeout prevents tail-latency spike from single slow tool

### Resilience
- **Recovery orchestrator** — 6 strategies: Retry → Reroute → Replan → Repair → Escalate → Degrade
- **Chaos testing** — 10 fault injection types (timeout, partition, crash, corruption, rate-limit, etc.)
- **Circuit breaker** — state-machine based fail-fast with cooldown
- **Hot failover** — primary-to-fallback model switching with blacklist cooldown

### Observability
- **governance.status endpoint** — real p95 latency, DAG width/depth, cache metrics, idempotency conflict rate
- **OpenTelemetry tracing** — spans for request routing, tool execution, and agent selection
- **Audit replay** — full task execution evidence chain, reproducible and filterable

### Session Management
- **Session context manager** — key concept extraction, message importance scoring, continuity markers
- **Session compression** — semantic compression of overflow messages, keeping recent + system + directive messages
- **Context window budget** — intelligent message retention when exceeding token limits

### Configuration & Deployment
- **Hot-reload config** — file-watch based, atomically swaps active config at runtime
- **Schema versioning** — semver-tracked config versions with forward/backward migration
- **3 build profiles** — local (SQLite), simple-server (SQLite), multi-users-server (PostgreSQL + pgvector)
- **Keyring integration** — system-native secret storage (macOS Keychain, Linux Secret Service, Windows Credential Manager)

---

## Architecture

go-on uses a **14-bus capability architecture** with 21 cognitive (F-GAP) modules:

```
┌──────────────────────────────────────────────────────────┐
│                     HarnessBus (Governance)              │
│  Policy · Drift · Resilience · Security · Audit         │
├──────────────────────────────────────────────────────────┤
│                   CapabilityBus (Intelligence)            │
│  Sense → Decide → Act → Feedback → Evolve               │
├──────────┬──────────┬──────────┬──────────┬─────────────┤
│ ToolBus  │ObservB.  │OptimizB. │MemoryBus │ProtocolBus  │
├──────────┼──────────┼──────────┼──────────┼─────────────┤
│OrchestB. │          │          │DistMemB. │             │
└──────────┴──────────┴──────────┴──────────┴─────────────┘
```

### Key Capability Modules

| Module | Description |
|:-------|:------------|
| **Planner** | Task-adaptive DAG planning with dependency inference |
| **DAG Driver** | Topological execution with parallel group scheduling |
| **BrainLoop** | Plan → Execute → Reflect → Replan cognitive cycle |
| **CapabilityBus** | Multi-factor agent selection (reputation + recency + task-fit + outcome) |
| **SelfModelCore** | System self-awareness and capability tracking |
| **MetacognitiveController** | Observation-driven reflection and corrective action |
| **WorldModel** | Entity/event/relationship tracking pipeline |
| **FederatedRL** | Distributed reinforcement learning across nodes |
| **DriftProtection** | Goal/capability/behavior drift detection |
| **HyperResilience** | Circuit breaker, failover group, self-healing |
| **MultiChannelTransport** | QoS-aware, deduplicated, prioritized message transport |

---

## Extensions

### VS Code Addon
The `vscode-addon/` directory contains a VS Code extension that launches the go-on runtime and exposes 60+ commands — chat, workflow execution, skill management, and configuration — directly within the editor.

```bash
cd vscode-addon
npm install
npm run compile
```

### SDKs
- **Rust SDK** (`sdk/rust/`) — Strongly typed client for go-on ACP/MCP endpoints, 40+ methods across 8 domains
- **Python SDK** (`sdk/python/`) — HTTPX-based client with streaming support and `py.typed` markers

---

## Build Profiles

| Profile | Backend | Use Case | Build Command |
|:--------|:--------|:---------|:--------------|
| `profile-local` | SQLite + sqlite-vec | Single-user local tool | `cargo build` (default) |
| `profile-simple-server` | SQLite + sqlite-vec | Single-server deployment | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | Multi-user production | `cargo build --no-default-features -F profile-multi-users-server` |

---

## Verification

| Profile | cargo check | cargo clippy `-D warnings` | Tests |
|:--------|:-----------:|:--------------------------:|:-----:|
| `profile-local` | ✅ 0 errors | ✅ 0 warnings | 800+ |
| `profile-simple-server` | ✅ 0 errors | ✅ 0 warnings | 900+ |
| `profile-multi-users-server` | ✅ 0 errors | ✅ 0 warnings | 1,000+ |

---

## License

MIT License — see [LICENSE](LICENSE) for details.
