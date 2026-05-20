# go-on

English | [简体中文](README.zh-CN.md)

**go-on** is a Rust runtime for **ACP/MCP-oriented agent orchestration, governance, and production-safe operations** — your all-in-one AI agent runtime.

- 🖥️ **Desktop GUI** — monitor, chat, skills & tools management
- 🧠 **14-bus intelligence core** — CapabilityBus + HarnessBus closed-loop governance
- 🌐 **Full i18n** — English, Simplified Chinese, Traditional Chinese (448+ keys each)
- 🛡️ **Safeguard Mode** — AI-powered risk assessment for high-stakes decisions
- 🔌 **Multi-Protocol** — ACP + MCP, stdio + HTTP, single-user to multi-user cluster
- 🤖 **35+ AI providers** — OpenAI, Anthropic, DeepSeek, Gemini, Groq, Ollama and more

---

## GUI Desktop App

The EGUI-based desktop GUI (`gui/`) provides real-time monitoring, multi-session chat, skills management, and visual configuration — no terminal needed.

```bash
cargo run --manifest-path gui/Cargo.toml
```

### Screenshots

| Monitor Dashboard | Chat Interface |
|:---:|:---:|
| ![Monitor](snapshots/monitor.png) | ![Chat](snapshots/chat.png) |

| Provider Management | Settings |
|:---:|:---:|
| ![Providers](snapshots/providers.png) | ![Settings](snapshots/settings.png) |

| Skills & Tools |
|:---:|
| ![Skills](snapshots/skills.png) |

### GUI Features
- **Monitor tab**: Backend health, AI provider status, real-time metrics
- **Chat tab**: Multi-session conversations with phase/mode selection, file attachments, multi-model support, automatic message pruning (max 1000/session), dynamic AI status indicators
- **Skills tab**: Create and manage AI skills with built-in skill-creator
- **Settings tab**: Provider management (35 providers), GUI config editor, 6 themes, language switching (en/zh-CN/zh-TW)
- **Risk Decision Panel**: When the backend detects high-risk topics (medical, legal, financial, security, etc.), a **Risk Decision panel** shows the risk score, strategy (multi-model vote, multi-agent vote, escalation), review requirements, and specific reasons — enabling informed human oversight
- **Keyring**: Dual storage (system keyring + config file)
- **Auto-restart**: Backend auto-restarts on crash with exponential backoff (3→96s)

---

## Architecture: Multi-Bus Capability System

go-on implements a **14-bus architecture** centered on `CapabilityBus` and `HarnessBus`:

### Core Buses
| Bus | Description |
|:----|-------------|
| **CapabilityBus** | Central intelligence bus; orchestrates sense → decide → act → feedback → evolve |
| **HarnessBus** | Governance entry; policy evaluation, drift/resilience/security checks |

### Sub-Buses
| Bus | Description |
|:----|-------------|
| **ToolBus** | Unified tool/skill invocation, capability matrix, agent-tool matching |
| **ObservabilityBus** | Latency, error rates, agent health |
| **OptimizationBus** | Cost/speed/reliability recommendation, circuit breaker |
| **MemoryBus** | Cascading cache (L1 memory → L2 SQLite → L3 vector store) |
| **ProtocolBus** | Protocol-aware routing, health/latency tracking |
| **OrchestrationBus** | Flow/mode/router orchestration, mode recommendation |
| **DistributedMemoryBus** | Cross-node memory sharing (multi-user profile) |

### F-GAP Cognitive Modules (21/21 Complete ✅)

| Module | Description |
|:-------|-------------|
| OmnipotentMode | Self-healing task execution |
| BrainLoop | Plan → Execute → Reflect → Replan |
| ConsensusEngine | Multi-agent voting governance |
| SelfModelCore | System self-awareness & capability tracking |
| ConsciousnessMetrics | Agent consciousness state machine |
| MetacognitiveController | Observation-driven reflection & action |
| WorldModel | Entity/event/relationship pipeline |
| DiscoveryCenter | Cross-session pattern mining |
| EvolutionGraph | Capability lifecycle & trend tracking |
| FederatedRL | Distributed reinforcement learning |
| DriftProtection | Goal/capability/behavior drift detection |
| HyperResilience | Circuit breaker, failover, self-healing |
| FaultTolerance | Cross-node fault isolation & auto-recovery |
| MultiChannelTransport | QoS-aware, deduplication, message peek |

### 38-Dimensional Full Star Rating

```
Governance & Compliance (5/5):    ★★★★★ Provenance, Drift, Policy, Token, Security
Resilience & Fault Tolerance (2/2):★★★★★ HyperResilience, FaultTolerance
Orchestration & Execution (6/6):  ★★★★★ OrchestrationBus, Scheduler, ExecutionGraph,
                                        OmnipotentMode, ArtifactLayer, BrainLoop
Routing & Scheduling (7/7):       ★★★★★ CapabilityGraph, Reputation, QLearning,
                                        ScenarioMatcher, Discovery, WorkflowRegistry, AgentFactory
Protocol & Transport (2/2):       ★★★★★ ProtocolBus, MultiChannelTransport
Memory & Cache (2/2):             ★★★★★ MemoryBus, DistributedMemoryBus
Observability & Optimization (3/3):★★★★★ ObservabilityBus, OptimizationBus, ToolBus
Intelligent Cognition (5/5):      ★★★★★ Knowledge Distillation, Deep RL, Skill Retention,
                                        AI Evolution, Self-built Skills
Self-Cognition (5/5):             ★★★★★ SelfModel, Consciousness, Metacognitive,
                                        WorldModel, Consensus
───────────────────────────────────────────────────────────────────────────────────
Total (38/38):                    100% ★★★★★
```

---

## Runtime Protocol Modes

5 modes for any integration scenario:

| Mode | Description |
|:-----|-------------|
| `adaptive` (default) | Dual-stack protocol, request-type aware routing |
| `acp_stdio` / `acp_http` | ACP over stdio or HTTP |
| `mcp_stdio` / `mcp_http` | MCP over stdio or HTTP |

Example config:
```toml
[protocol]
mode = "adaptive"
```

---

## Internationalization (i18n)

Full i18n coverage (~95%) across the entire backend:

| Language | File | Keys |
|:---------|:-----|:----:|
| English (US) | `languages/en_US.json` | 448+ |
| Chinese (Simplified) | `languages/zh_CN.json` | 448+ |
| Chinese (Traditional) | `languages/zh_TW.json` | 448+ |

Coverage: ACP/MCP HTTP errors ✅, Agent provider modules ✅, Config validation ✅, CLI setup ✅, API handler errors ✅, Orchestration ✅

---

## Quick Start

```bash
# Clone & run (auto-creates config if none found)
git clone https://github.com/your-org/go-on
cd go-on
cargo run

# Or start the desktop GUI
cargo run --manifest-path gui/Cargo.toml

# Terminal chat mode (like Claude Code / Codex)
go-on --chat
```

First run auto-detects your environment — if no AI providers are configured, the setup wizard will guide you interactively.

Default health endpoint: `http://127.0.0.1:8090/health`

---

## License

MIT or BSD (your choice).
