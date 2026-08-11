# Backend CLI

The backend executable is the authoritative control surface for runtime startup, setup, health, planning, and transport mode selection.

## Invocation forms

Production or packaged binary:

```bash
go-on --config config.toml
```

During development:

```bash
cargo run -- --config config.toml
```

Current help summary:

```text
Usage: go-on [OPTIONS] [COMMAND]
```

Most operations are driven by flags; a small set of subcommands is also available: `init`, `status`, `diagnose`, `skill`, and `hub` (feature-gated).

## Core runtime options

### `--config <CONFIG>`

Use an explicit configuration file path. If omitted, the runtime resolves `config.toml` in this order: `./config.toml` (current directory), then `~/.config/go-on/config.toml` (Windows: `%APPDATA%\go-on\config.toml`), then the executable directory as a last resort.

Example:

```bash
go-on --config D:\go-on\config.toml
```

## Phase & Sub-Phase Configuration

Phases define the workflow stages the runtime executes. Each phase can optionally contain sub-phases for finer-grained control.

### Phase configuration in config.toml

Phases are configured under `[phases.<name>]` sections, referenced by the `flow.phases` list:

```toml
[flow]
# Order of execution. Add or remove phases to match your workflow.
phases = ["think", "act", "check", "done"]

[phases.think]
description = "Think — analyze, plan, gather context"
# Agents assigned to this phase (empty = setup wizard will prompt)
agents = []
# If true, the phase continues even when no agents are configured
fallback = true

[phases.think.options]
request_timeout_seconds = 120
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
summary_enabled = true
phase_max_inflight = 8      # max concurrent tasks within this phase
global_max_inflight = 128    # max concurrent tasks across all phases
```

### Sub-phases

Sub-phases provide hierarchical workflow decomposition. A phase can define a `sub_phases` list with nested `[phases.<parent>.<child>]` sections:

```toml
[flow]
phases = ["think", "act", "check", "done"]

[phases.act]
description = "Main execution phase"
agents = []
fallback = true
# Sub-phases run in order within this phase
sub_phases = ["plan", "code", "test"]

[phases.act.options]
request_timeout_seconds = 300
cache_enabled = true
phase_max_inflight = 24

[phases.act.plan]
description = "Implementation plan"
agents = []
fallback = true

[phases.act.plan.options]
request_timeout_seconds = 120
phase_max_inflight = 4

[phases.act.code]
description = "Write code"
agents = []
fallback = true

[phases.act.code.options]
request_timeout_seconds = 180
phase_max_inflight = 12

[phases.act.test]
description = "Run tests"
agents = []
fallback = true

[phases.act.test.options]
request_timeout_seconds = 120
phase_max_inflight = 8
```

Sub-phases inherit their parent's `options` as defaults, which can be overridden per sub-phase.

### Phase-only vs sub-phase execution

- **Without sub-phases**: each phase runs top-to-bottom in the `phases` list order.
- **With sub-phases**: the parent phase orchestrates its sub-phases in order before moving to the next parent phase.
- Sub-phases are optional — most workflows work fine with flat phases only.

### Built-in phase preset files

Four preset config files ship with the project — each with a different phase setup:

| File | Phases | Best for |
|------|--------|----------|
| `config.toml` | think, act, check, done | Generic workflows (default) |
| `zed-config.toml` | think, act, check, review, done | IDE integrations (Zed, VS Code) |
| `config.simple-server.toml` | think, act, check, done | Single-server deployment |
| `config.multi-users-server.toml` | think, act, check, done | Multi-user enterprise |

Note: when no config file exists at all, the runtime writes bootstrap defaults whose flow is `planning` → `coding` → `review` → `delivery` (see `src/core/config/defaults.rs`); the shipped preset files above use `think`/`act`/`check`/`done` (+`review` for Zed).

### Using a specific phase config

```bash
# Use the Zed config preset with an IDE
# (config/zed-config.toml — the shipped IDE-oriented preset)
go-on --config zed-config.toml

# Use the universal config with HTTP endpoint
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

### Creating custom phases

You can define any phase name — there are no built-in restrictions:

```toml
[flow]
phases = ["research", "draft", "review", "approve", "publish"]

[phases.research]
description = "Gather information and sources"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
cache_enabled = true
vector_enabled = true
summary_enabled = true
phase_max_inflight = 4
```

### Key options per phase

| Option | Default | Description |
|--------|---------|-------------|
| `request_timeout_seconds` | 150 | Max time for a single task request within this phase |
| `review_timeout_seconds` | 60 | Max time for review within this phase |
| `review_timeout_policy` | `"reject"` | Action on review timeout (`"reject"`, `"degrade_single"`, or `"warn"`) |
| `review_min_response_chars` | 12 | Minimum characters expected in a review response |
| `cache_enabled` | true | Enable cache lookups within this phase |
| `vector_enabled` | true | Enable vector store lookups within this phase |
| `summary_enabled` | true | Enable conversation summarization |
| `phase_max_inflight` | 24 | Max concurrent tasks within this phase |
| `global_max_inflight` | 128 | Max concurrent tasks across all phases globally |
| `autopilot_complexity` | `"auto"` | Complexity mode: `"auto"`, `"simple"`, `"complex"` |

## Validation and readiness

### `--validate-config` or `--doctor`

Validate configuration and exit. This is the fastest cheap check before debugging a larger runtime problem.

```bash
go-on --config config.toml --validate-config
```

### `--status` or `--check`

Print configured AI providers and runtime readiness status.

Use this after setup, after editing `config.toml`, or before attaching an editor client.

```bash
go-on --status
```

### `--healthcheck`

Generate a runtime health report and persist it under `.goon/`. Use this when you need a durable artifact for later review or triage.

```bash
go-on --healthcheck
```

## Setup and recommendation workflow

### `--setup` or `--init`

Run the interactive setup wizard.

```bash
go-on --setup
```

### `--setup-profile <SETUP_PROFILE>`

Current accepted value: `adaptive`.

Example:

```bash
go-on --setup --setup-profile adaptive
```

### `--setup-level <SETUP_LEVEL>`

Accepted values:

- `quick`
- `standard`
- `custom`

Practical guidance:

- `quick`: minimal path, skips extra-agent prompting.
- `standard`: best default for most users.
- `custom`: exposes more manual decisions.

### `--setup-secrets <SETUP_SECRETS>`

Accepted values:

- `env`
- `keyring`
- `auto`

`auto` also accepts `autodetect` internally.

### `--apply-recommended`

Apply provider-capability recommendations to the current `config.toml` and exit.

Use this after onboarding providers or after changing the model mix.

### `--force`

Force setup even if target files already exist.

Use it carefully, especially when you intentionally maintain a hand-edited `config.toml`.

## Local model registration

### `--add-local-model` or `--add-model`

Add or update a local model agent entry in config.

This flag is typically combined with the related `--local-model-*` options below.

### `--local-model-name <NAME>`

Logical agent name.

### `--local-model-url <URL>`

Endpoint URL for the local provider.

### `--local-model-type <TYPE>`

Provider type. Default intent is `openai`.

### `--local-model-model <MODEL_ID>`

Model identifier to store in config.

### `--local-model-api-key-env <ENV_NAME>`

Optional API-key environment-variable field.

### `--local-model-secret-key-env <ENV_NAME>`

Optional secret-key environment-variable field.

### `--local-model-register-only`

Register the local model under `[agents]` only, without auto-attaching it to phase agent lists.

Example:

```bash
go-on --add-local-model \
  --local-model-name ollama-local \
  --local-model-url http://127.0.0.1:11434/v1 \
  --local-model-type openai \
  --local-model-model qwen2.5-coder \
  --local-model-register-only
```

## Secret management

### `--secret <ACTION>`

Accepted actions:

- `set`
- `get`
- `delete`
- `list`

### `--secret-name <SECRET_NAME>`

Name of the logical secret target.

### `--secret-value <SECRET_VALUE>`

Secret value used with `set`.

Examples:

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
go-on --secret get --secret-name openai
go-on --secret delete --secret-name openai
```

## Planning and artifact checks

### `--action-check <ACTION_CHECK>`

Run action checks against `.goon/` artifacts.

Expected values are described in help as:

- `all`
- `spec`
- `qa`
- `retest`
- `final`

### `--plan-task <PLAN_TASK>`

Build and persist a controlled task-plan artifact for a complex task.

Use this when you want the runtime to materialize a durable plan object before execution.

## Transport mode selection

### `--protocol-mode <MODE>`

Accepted values:

- `adaptive` (recommended default)
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

Recommended usage:

- `adaptive`: safest default when multiple clients may attach; it preserves dual-stack request routing and derives the startup transport from runtime prerequisites.
- `acp_stdio`: best when an editor launches `go-on` as a child process.
- `acp_http`: best when an ACP-compatible client wants one shared long-running backend.
- `mcp_stdio`: use only when your client explicitly expects MCP over stdio.
- `mcp_http`: best when your client wants OpenAI-compatible `/v1` HTTP endpoints.

### `--acp-http-bind <ADDR>`

Bind an HTTP listener and expose:

- `/health`
- `/chat`
- `/chat/stream`

In practice the same runtime also exposes the OpenAI-compatible `/v1` endpoints used by Zed model-provider style integrations and by runtime probes.

Example:

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

## Terminal Chat Mode (`--chat` / `-a`)

Start an interactive terminal chat session (like Claude Code or Codex).

```bash
go-on -a
# or
go-on --chat
```

If your config file is in a different location:

```bash
go-on -c /path/to/config.toml -a
```

### Requirements

At least one AI provider must be configured in `config.toml` with valid API keys. API keys are read automatically from the system keyring (keyring → env var fallback).

### Behavior

1. Builds the agent registry from the configured providers.
2. Opens a readline-style chat loop.
3. Each message is sent to the first available agent with streaming output.
4. Supports conversation history (capped at 1000 messages).
5. Handles graceful shutdown on Ctrl+C or `/quit`.

### Built-in Commands

| Command | Description |
|---------|-------------|
| `/quit` or `/exit` | Exit chat mode |
| `/help` | Show available commands |
| `/clear` | Clear conversation history |
| `/agents` | List configured agents |

### Automatic Setup Redirect

If `--chat` is passed and no providers are configured, the onboarding prompt is skipped and a message directs you to run `--setup` first:

```bash
go-on -c config.toml -a
# → "No AI agents configured. Run go-on --init to set up a provider first."
```

## Common command recipes

```bash
go-on --setup --setup-level standard --setup-secrets auto
```

Validate then inspect readiness:

```bash
go-on --config config.toml --validate-config
go-on --config config.toml --status
```

Start a shared local HTTP runtime for GUI, Zed, and probes:

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

Run ACP over stdio for an editor-launched integration:

```bash
go-on --config config.toml --protocol-mode acp_stdio
```

Terminal chat (interactive, like Claude Code):

```bash
go-on -a
```

## Operational guidance

- Use `--validate-config` before assuming the transport layer is broken.
- Use `--status` before opening the GUI or an editor plugin.
- Use `adaptive` unless you have a concrete client contract that requires ACP-only or MCP-only behavior.
- Prefer `--add-local-model` over hand-editing config when onboarding a local OpenAI-compatible endpoint.
