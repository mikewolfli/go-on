# go-on Workflow Configuration Guide

go-on supports multiple workflow types, each designed for different use cases.
Workflows are configured in the `[flow]` section of `config.toml`.

---

## 1. Workflow Types

| Type | Value | Use Case | Default Phases |
|------|-------|----------|----------------|
| Auto | `"auto"` | Auto-detect based on context | varies |
| Dev | `"dev"` | Software development | planning → coding → review → delivery |
| General | `"general"` | Q&A, analysis, research | gathering → thinking → executing → validating → closing |
| Free | `"free"` | Single-turn, no phase routing | none |
| Custom | `"custom"` | User-defined phases | user-defined |

---

## 2. Quick Start Templates

Ready-to-use config presets ship in the `config/` directory. Start from one of them:

### Universal Workflow (default)
```bash
cp config/config.toml config.toml
# Edit config.toml to add your API keys
go-on --config config.toml
```

### Low-Memory Workflow
```bash
cp config/config.low-memory.toml config.toml
# Edit config.toml to add your API keys
go-on --config config.toml
```

---

## 3. How Agent Routing Works

**Agents are NOT bound to phases.** All registered AI providers are available
to the system globally. The **CapabilityBus** dynamically selects the best
provider for each subtask based on:

1. **Task context** — what type of work is being done (coding, analysis, etc.)
2. **Reputation score** — historical success rate for similar tasks
3. **Capability tags** — what each provider is good at

This means you can define phases without listing agents:

```toml
[phases.coding]
description = "Coding — implement features"
agents = []          # ← Empty! The capability bus picks the best agent.
fallback = true
```

Or you can give the bus hints by listing preferred agents:

```toml
[phases.coding]
agents = ["deepseek", "openai"]   # ← Hint: prefer these, but bus still decides
fallback = true
```

When `agents` is empty or contains multiple names, the CapabilityBus
selects the best one at runtime.

---

## 4. Defining a Custom Workflow (`workflow_type = "custom"`)

You can define your own workflow phases and transitions.
Here is a complete example:

```toml
default_phase = "research"
model_selection_mode = "adaptive"

[flow]
name = "My Research Workflow"
workflow_type = "custom"
phases = ["research", "draft", "polish", "publish"]

# ── Phase Definitions ──────────────────────────────────────────
# Each phase can have its own timeout, cache, and agent preferences.
# Leave agents = [] to let the capability bus decide.

[phases.research]
description = "Research — gather sources, analyze data"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
review_timeout_seconds = 90
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128

[phases.draft]
description = "Draft — write initial content"
agents = []
fallback = true

[phases.draft.options]
request_timeout_seconds = 300
review_timeout_seconds = 120
cache_enabled = true
vector_enabled = true
phase_max_inflight = 24
global_max_inflight = 128

[phases.polish]
description = "Polish — refine, edit, improve"
agents = []
fallback = true

[phases.polish.options]
request_timeout_seconds = 180
review_timeout_policy = "reject"
review_min_response_chars = 12
cache_enabled = true
vector_enabled = true
phase_max_inflight = 16
global_max_inflight = 128

[phases.publish]
description = "Publish — finalize and deliver"
agents = []
fallback = false

[phases.publish.options]
request_timeout_seconds = 90
phase_max_inflight = 8
global_max_inflight = 64
```

---

## 5. Phase Options Reference

Each `[phases.<name>.options]` block supports:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `request_timeout_seconds` | u64 | 150 | Max seconds per request (adaptive coding template) |
| `review_timeout_seconds` | u64 | 60 | Max seconds for review (adaptive coding template) |
| `review_timeout_policy` | string | `"reject"` | `"reject"`, `"degrade_single"`, or `"warn"` |
| `review_min_response_chars` | usize | 12 | Min chars for review to accept |
| `cache_enabled` | bool | true | Enable response cache |
| `vector_enabled` | bool | true | Enable vector memory |
| `summary_enabled` | bool | false | Enable phase summary |
| `phase_max_inflight` | usize | 24 | Max concurrent tasks in this phase (adaptive coding template) |
| `global_max_inflight` | usize | 128 | Max concurrent tasks globally |
| `extra` | table | `{}` | Additional key-value options |

---

## 6. `workflow_type = "auto"` (Default)

When `workflow_type` is not set or set to `"auto"`, the system detects
the context at startup:

- If a code repository is detected → uses **Dev** workflow (4 phases)
- Otherwise → uses **General** workflow (5 phases)

You can override by explicitly setting `workflow_type` in your config.

---

## 7. `workflow_type = "free"` (No Phase Routing)

Free mode bypasses phase routing entirely. Every request is handled as
a single-turn interaction without phase transitions:

```toml
[flow]
name = "Free Chat"
workflow_type = "free"
phases = []   # No phases — direct routing only
```

In free mode, `default_phase` and `effective_default_phase()` both return
`None`, and requests are routed directly to the best available agent.

---

## 8. Config File Location

go-on looks for `config.toml` in this order:

1. `--config <path>` command-line flag
2. `./config.toml` (current directory)
3. `~/.config/go-on/config.toml` (XDG config on Linux/macOS)
4. `%APPDATA%/go-on/config.toml` (Windows)

Template files are located in `config/` in the project directory.

---

## 9. High-Risk Multi-Agent Voting

For high-risk phases (e.g., medical, legal, financial, security-critical code),
go-on supports **multi-agent joint processing** where multiple AI providers
independently generate responses and vote on the best result.

### How It Works

1. **Risk detection** — The system analyzes the user's message for domain
   keywords (medical, legal, financial, etc.) and decision keywords
   (delete, modify, approve, authorize, etc.).
2. **Multi-agent execution** — When risk exceeds the configured threshold,
   multiple agents (up to `high_risk_vote_max_agents`) independently
   generate responses in parallel.
3. **Voting** — Responses are deduplicated and ranked. The one with the
   most consensus wins.
4. **Escalation** — If no clear consensus (tie), additional models can be
   invoked to break the tie.

### High-Risk Options

These options can be set per-phase or globally via `extra`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `high_risk_vote_enabled` | bool | `true` | Enable high-risk detection |
| `high_risk_vote_threshold` | usize | `2` | Risk score threshold to trigger voting (matches keywords) |
| `high_risk_domain_keywords` | string[] | `["medical","legal","financial",...]` | Domain keywords that increase risk score |
| `high_risk_decision_keywords` | string[] | `["delete","modify","approve",...]` | Decision keywords that increase risk score |
| `high_risk_multi_agent_vote_enabled` | bool | `true` | Enable multi-agent voting when high risk |
| `high_risk_vote_min_agents` | usize | `2` | Minimum agents to vote (clamped 1-6) |
| `high_risk_vote_max_agents` | usize | `3` | Maximum agents to vote |
| `high_risk_escalate_multi_model_enabled` | bool | `true` | Enable escalation on tie |
| `high_risk_escalate_models_per_agent` | usize | `2` | Extra models per agent on escalation |
| `high_risk_escalate_max_agents` | usize | `3` | Max agents on escalation |

### Example: Multi-Agent Review Phase

```toml
[phases.review]
description = "Review — security-critical code review with multi-agent voting"
agents = []
fallback = true

[phases.review.options]
request_timeout_seconds = 180
review_timeout_seconds = 90
review_timeout_policy = "reject"
review_min_response_chars = 12
cache_enabled = true
vector_enabled = true
phase_max_inflight = 16
global_max_inflight = 128
high_risk_vote_enabled = true
high_risk_vote_threshold = 1     # Always trigger voting for review phase
high_risk_vote_min_agents = 2     # At least 2 agents must vote
high_risk_vote_max_agents = 4     # Up to 4 agents participate
high_risk_escalate_multi_model_enabled = true
```

### Example: Medical/Clinical Question (General Workflow)

```toml
[phases.executing]
description = "Executing — generate clinical analysis with multi-agent verification"
agents = []
fallback = true

[phases.executing.options]
request_timeout_seconds = 300
review_timeout_seconds = 180
cache_enabled = false          # Disable cache for sensitive data
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128
high_risk_vote_enabled = true
high_risk_vote_threshold = 2
high_risk_vote_min_agents = 3  # Three agents for critical decisions
high_risk_vote_max_agents = 5
high_risk_escalate_multi_model_enabled = true
high_risk_escalate_models_per_agent = 3
```

### Voting Flow

```
User sends high-risk query
        │
        ▼
┌─────────────────────────────┐
│  Risk Assessment            │
│  - Domain keyword check     │
│  - Decision keyword check   │
│  - Score ≥ threshold?       │
└─────────────┬───────────────┘
              │
      ┌───────┴───────┐
      ▼               ▼
  Low Risk        High Risk
  (single          │
   agent)          ▼
          ┌──────────────────┐
          │ Multi-Agent Vote  │
          │ Agent A → resp A  │
          │ Agent B → resp B  │
          │ Agent C → resp C  │
          └────────┬─────────┘
                   │
                   ▼
          ┌──────────────────┐
          │ Dedup & Rank      │
          │ - Normalize text  │
          │ - Count votes     │
          │ - Pick winner     │
          └────────┬─────────┘
                   │
          ┌────────┴────────┐
          ▼                 ▼
      Consensus?         No consensus?
          │                 │
          ▼                 ▼
     Return result    Escalation round
                      (more models)
                           │
                           ▼
                      Return best result
```

### Default High-Risk Keywords

**Domain keywords** that trigger risk detection:
`medical`, `diagnosis`, `clinical`, `prescription`, `treatment`, `surgery`,
`healthcare`, `legal`, `contract`, `litigation`, `financial`, `investment`,
`trading`, `compliance`, `security`, `authentication`, `authorization`,
`encryption`, `infrastructure`, `deployment`, `production`

**Decision keywords** that amplify risk:
`delete`, `drop`, `remove`, `modify`, `alter`, `override`, `approve`,
`authorize`, `grant`, `revoke`, `execute`, `deploy`, `release`, `publish`,
`terminate`, `shutdown`

These defaults can be overridden per-phase via `high_risk_domain_keywords`
and `high_risk_decision_keywords` options.

## 10. Skill System

The Skill System allows you to define reusable capabilities that can be
invoked across workflows. Skills encapsulate specific functionality with
their own configuration, phases, and agent preferences.

### Skill Discovery and Execution Flow

1. **Registration** — Skills are registered in `config.toml` under the `[skills]` section
2. **Discovery** — The SkillBus scans registered skills at startup
3. **Invocation** — Skills can be invoked explicitly by name or automatically matched based on context
4. **Execution** — Each skill runs in its own phase context with dedicated agents
5. **Result** — Skill output can be fed back into the main workflow or returned directly

### Skill Dedup Protection

The system automatically deduplicates skills by name and version:

- If two skills have the same name, the one with the higher version wins
- If versions are equal, the last registered skill is kept
- Built-in skills cannot be overridden by custom skills with the same name

### Creating Skills

Skills are defined in the `[skills]` section of `config.toml`:

```toml
[skills.code-review]
enabled = true
description = "Perform automated code review"

[skills.code-review.phases.coding]
description = "Analyze code changes"
agents = ["reviewer-agent"]
fallback = true
```

## 11. Feature Profiles

go-on has three build profiles that enable different feature sets:

| Feature | `local` (default) | `simple-server` | `multi-users-server` |
|---------|--------------------------|------------------------|------------------------------|
| SQLite backend | ✅ | ✅ | ❌ |
| PostgreSQL backend | ❌ | ❌ | ✅ |
| ToolBus | ✅ | ✅ | ✅ |
| OrchestrationBus | ✅ | ✅ | ✅ |
| ObservabilityBus | ✅ | ✅ | ✅ |
| OptimizationBus | ✅ | ✅ | ✅ |
| MemoryBus | ✅ | ✅ | ✅ |
| ProtocolBus | ✅ | ✅ | ✅ |
| DistributedMemoryBus | ❌ | ✅ | ✅ |

To build with a specific profile:

```bash
# Default (local)
cargo build

# Simple server
cargo build --no-default-features --features simple-server

# Multi-user server
cargo build --no-default-features --features multi-users-server
```

---

> 📖 本文档仅包含工作流配置相关内容。其他文档请参见：[文档目录](README.md)。
