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

Two ready-to-use config templates are provided:

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
| `request_timeout_seconds` | u64 | 120 | Max seconds per request |
| `review_timeout_seconds` | u64 | 60 | Max seconds for review |
| `review_timeout_policy` | string | `"reject"` | `"reject"` or `"warn"` |
| `review_min_response_chars` | usize | 12 | Min chars for review to accept |
| `cache_enabled` | bool | true | Enable response cache |
| `vector_enabled` | bool | true | Enable vector memory |
| `summary_enabled` | bool | false | Enable phase summary |
| `phase_max_inflight` | usize | 8 | Max concurrent tasks in this phase |
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

Template files are located in `config/templates/` in the project directory.
