# Workflow Configuration

go-on supports multiple workflow types designed for different use cases. Workflows are configured in the `[flow]` section of `config.toml`.

> Related docs: [GUI Console](gui.md) | [Prompts System](prompts.md)

---

## 1. Workflow Types

| Type | Value | Use Case | Default Phases |
|------|-------|----------|----------------|
| Auto | `"auto"` | Auto-detect based on context | Varies |
| Dev | `"dev"` | Software development | planning → coding → review → delivery |
| General | `"general"` | Q&A, analysis, research | gathering → thinking → executing → validating → closing |
| Free | `"free"` | Single-turn, no phase routing | None |
| Custom | `"custom"` | User-defined phases | User-defined |

---

## 2. Quick Start

Two ready-to-use config templates are available in `config/templates/`:

### Development Workflow
```bash
cp config/templates/config.dev.toml config.toml
# Edit config.toml to add your API keys
go-on --config config.toml
```

### General Workflow
```bash
cp config/templates/config.general.toml config.toml
# Edit config.toml to add your API keys
go-on --config config.toml
```

---

## 3. Agent Routing Flow

**Agents are NOT bound to phases.** All registered AI providers are available globally. The **CapabilityBus** dynamically selects the best provider for each subtask based on:

1. **Task context** — what type of work is being done (coding, analysis, etc.)
2. **Reputation score** — historical success rate for similar tasks
3. **Capability tags** — what each provider is good at

You can define phases without listing agents — the bus picks the best one automatically:

```toml
[phases.coding]
description = "Coding — implement features"
agents = []          # ← Empty! The capability bus picks the best agent.
fallback = true
```

Or give the bus hints by listing preferred agents:

```toml
[phases.coding]
agents = ["deepseek", "openai"]   # ← Hint: prefer these, but bus still decides
fallback = true
```

---

## 4. Auto-detect (`workflow_type = "auto"`)

When `workflow_type` is not set or set to `"auto"`, the system detects the context at startup:

- If a code repository is detected → uses **Dev** workflow (4 phases)
- Otherwise → uses **General** workflow (5 phases)

Override by explicitly setting `workflow_type` in your config.

---

## 5. Free Mode (`workflow_type = "free"`)

Free mode bypasses phase routing entirely. Every request is a single-turn interaction without phase transitions:

```toml
[flow]
name = "Free Chat"
workflow_type = "free"
phases = []   # No phases — direct routing only
```

---

## 6. Custom Workflow

Define your own workflow phases and transitions:

```toml
default_phase = "research"
model_selection_mode = "adaptive"

[flow]
name = "My Research Workflow"
workflow_type = "custom"
phases = ["research", "draft", "polish", "publish"]

[phases.research]
description = "Research — gather sources, analyze data"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128
```

Each phase supports configurable options including timeout, caching, vector memory, concurrency limits, and high-risk multi-agent voting settings.

---

## 7. High-Risk Multi-Agent Voting

For high-risk phases (medical, legal, financial, security-critical), go-on supports multi-agent joint processing:

1. **Risk detection** — Analyzes user message for domain and decision keywords
2. **Multi-agent execution** — Multiple agents independently generate responses in parallel
3. **Voting** — Responses are deduplicated and ranked; consensus wins
4. **Escalation** — If no consensus, additional models break the tie

Configure per-phase via `options`:

```toml
[phases.review.options]
high_risk_vote_enabled = true
high_risk_vote_threshold = 1
high_risk_vote_min_agents = 2
high_risk_vote_max_agents = 4
high_risk_escalate_multi_model_enabled = true
```

---

> Full documentation including phase options reference, feature profiles, and skill system can be found in the project's `docs/workflow-config.md`.
