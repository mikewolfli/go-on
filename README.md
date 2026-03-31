# go-on

Rust ACP proxy for Zed with configurable flow, phases, per-phase principles, and multi-agent fallback routing.

## Development Rules

Repository engineering rules are documented in `DEVELOPMENT_RULES.md`.
Contributors should follow it for coding, testing, protocol compatibility, and documentation updates.

## Features

- JSON-RPC 2.0 over stdin/stdout for ACP-style chat handling.
- Flow and phase definitions loaded from `config.toml`.
- Supports `copilot`, `deepseek`, `wenxin`, `openai_compatible`, `doubao`, and `claude` agent types.
- Phase-aware principle injection:
  - DeepSeek/Wenxin: principles go to `system` message.
  - Copilot: principles are injected into a prefixed user instruction.
- Fallback routing through phase agent list when enabled.
- Retry with exponential backoff (up to 2 retries) for model calls.
- Optional SQLite exact-response cache for repeated requests.
- Optional SQLite vector memory with threshold-based retrieval.
- Optional per-phase rolling summary memory for long conversations.
- Built-in runtime metrics for cache/vector/summary effectiveness plus Prometheus export.
- Runtime health and maintenance RPCs: `runtime.health`, `maintenance.gc`, `cache.clear`, `vector.clear`.
- Runtime diagnostics and control RPCs: `breaker.status`, `breaker.reset`, `phase.status`, `config.reload`, `shutdown`.
- Chat modes compatible with Copilot-style workflows: `ask`, `edit`, `agent`, and `full_auto`.
- `full_auto` mode requires unanimous approval from 2 review agents before execution.

## Build

```bash
cargo build --release
```

## Run

```bash
cp config.toml.example config.toml
./target/release/go-on --config ./config.toml --verbose
```

CLI options:

- `--config <PATH>`: custom config file path.
- `--phase <PHASE>`: force all requests to use one phase (testing helper).
- `--verbose`: enable debug logs.
- `--validate-config`: validate config structure, required agent environment variables, external keyring references, and print a scored health report with severity-tagged warnings.
- `--setup`: first-run wizard. Generates config from template, initializes RULES files, and can store API keys into system keyring.
- `--setup-profile <simple|complex>`: run setup non-interactively with a preset profile.
- `--setup-secrets <env|keyring>`: choose whether generated config uses env vars or system keyring references.
- `--force`: allow setup to overwrite an existing config file.
- `--secret <set|get|delete|list>`: manage supported secrets in the system keyring.
- `--secret-name <NAME>`: secret name used with `--secret set|get|delete`.
- `--secret-value <VALUE>`: secret value used with `--secret set`.

First-run quick start:

```bash
./target/release/go-on --setup
```

Non-interactive examples:

```bash
./target/release/go-on --setup --setup-profile simple --setup-secrets keyring --force
./target/release/go-on --secret set --secret-name deepseek_api_key --secret-value "$DEEPSEEK_API_KEY"
./target/release/go-on --secret list
```

## Environment Variables

Configure model keys in your shell before launching:

```bash
export DEEPSEEK_API_KEY="your_deepseek_key"
export WENXIN_API_KEY="your_wenxin_api_key"
export WENXIN_SECRET_KEY="your_wenxin_secret_key"
```

Startup now fails fast when a configured agent depends on a missing or empty environment variable. Use `--validate-config` to check this without starting the ACP server.

For keyring-backed secrets, startup and `config.reload` also verify that every `keyring://...` reference resolves to a non-empty secret.

You can also store secrets in system keyring and reference them in config with:

`keyring://<service>/<account>`

Example:

- `api_key_env = "keyring://go-on/deepseek_api_key"`
- `secret_key_env = "keyring://go-on/wenxin_secret_key"`

On Windows, this maps to Credential Manager via the OS keyring backend.

## Chat Modes

The proxy supports four interaction modes aligned with GitHub Copilot's workflow plus an AUTOPILOT strategy:

### Mode â†?Approval Strategy Mapping

| Mode | Strategy | Behavior |
|------|----------|----------|
| **ask** | DEFAULT APPROVALS | User sees suggestion and must explicitly approve |
| **edit** | BY PASS APPROVAL | Direct edit suggestion; AI auto-executes without barrier |
| **agent** | BY PASS APPROVAL | Autonomous multi-step reasoning; AI chains steps automatically |
| **full_auto** | AUTOPILOT | Fully automatic; complexity level determines dual-review requirement |

### Autopilot Complexity (for `full_auto` mode)

- **Simple** (default): 2-3 step workflows, each step uses one dedicated AI agent; no dual-review overhead
- **Complex**: explicit multi-phase flow; `review` is a fixed 2-AI approval gate, while other phases can use additional AIs

Complex mode operational rule:

- Configure at least 2 independent review-capable AIs.
- Put those 2 reviewers in `full_auto_review_agents`.
- If you do not have 2 review AIs available, do not use complex mode; use simple mode instead.

Configure complexity in `[phases.<name>.options]`:

```toml
[phases.coding.options]
autopilot_complexity = "simple"  # or "complex"
```

### Request Format

Include `mode` in your chat request:

```json
{
  "jsonrpc": "2.0",
  "id": "123",
  "method": "chat",
  "params": {
    "mode": "full_auto",
    "messages": [ ... ]
  }
}
```

If `mode` is omitted, approval behavior defaults to `default_approvals`, and phase selection falls back to `default_phase` unless an explicit mode-phase mapping applies.

## Configuration


Copy and customize `config.toml.example`.

### Automatic Rule Loading (No Manual Prompt Wiring)

When config is loaded (startup or `config.reload`), phase principles are automatically extended from optional markdown files near `config.toml`.

Discovery order:

1. `RULES.md`
2. `RULES/global.md`
3. `RULES/common.md`
4. `RULES/local.md`
5. `<phase>.rules.md`
6. `RULES/<phase>.md`
7. `RULES/<phase>.rules.md`
8. `RULES/<phase>.local.md`

Merge behavior:

- Existing `phases.<name>.principles` in TOML are preserved.
- Auto-loaded rules are appended and deduplicated.
- Headings and fenced code blocks are ignored while parsing rule lines.
- Empty or comment-only rule files are ignored, but `--validate-config` and `config.reload` report them as warnings.

Template files are provided in `RULES/` and can be customized per project.

Important parts:

- `default_phase`: used when request has no `phase` field.
- `[cache]` (optional): local SQLite cache settings.
  - `enabled`: turn cache on/off.
  - `path`: sqlite file path (relative path resolves from config directory).
  - `default_ttl_seconds`: default cache TTL.
  - `max_entries`: upper bound of cache rows (oldest-updated rows are evicted).
- `[vector]` (optional): local SQLite vector memory settings.
  - `enabled`: turn vector memory on/off.
  - `auto_mode`: auto-enable retrieval only for sufficiently complex requests.
  - `path`: sqlite file path for vector memory.
  - `dimensions`: hashed embedding dimensions.
  - `min_query_chars`: minimum query length for retrieval.
  - `top_k`: max number of retrieved snippets.
  - `min_similarity`: minimum cosine threshold.
  - `max_snippet_chars`: max chars per injected snippet.
  - `max_entries`: upper bound of vector records.
  - `summary_enabled`: enable per-phase rolling summary memory.
  - `summary_trigger_messages`: inject summary only when message count reaches this threshold.
  - `summary_max_chars`: cap persisted summary length.
- `[flow].phases`: ordered phase list.
- `[autotune]` (optional): runtime tuning configuration for adaptive retrieval behavior.
  - `enabled`: turn adaptive tuning on/off.
  - `evaluate_interval`: requests per phase before one tuning round.
  - `min_query_chars_step`: step size when adjusting retrieval threshold.
  - `min_query_chars_min` / `min_query_chars_max`: clamp range for dynamic threshold.
  - `max_top_k`: upper bound when tuner expands retrieval breadth.
  - `low_precision_threshold` / `high_precision_threshold`: precision bands used for tightening or relaxing retrieval.
  - `state_path`: local JSON file path for persisted autotune state.
  - `cooldown_windows`: hold-off windows after each adjustment to reduce oscillation.
  - `min_vector_searches`: minimum vector-search sample size per window before precision-based adjustment.
  - `summary_trigger_min` / `summary_trigger_max`: clamp range for dynamic summary trigger.
- `[runtime]` (optional): server lifecycle and background maintenance settings.
  - `maintenance_interval_seconds`: interval for background cache cleanup.
  - `health_interval_seconds`: interval for periodic runtime health logging.
  - `shutdown_drain_seconds`: maximum grace period to drain in-flight requests after shutdown starts.
- `[phases.<name>]`:
  - `agents`: tried in order.
  - `fallback = true/false`: when true, continue to next agent on failure.
  - `principles`: injected into prompt for current phase.
  - `options`: strongly typed runtime controls plus pass-through agent overrides (for example review `stage`, provider `temperature`, or `max_tokens`).
  - Cache-related options (optional):
    - `cache_enabled`: enable/disable cache for this phase.
    - `cache_ttl_seconds`: override default cache TTL for this phase.
  - Vector-related options (optional):
    - `vector_enabled`: enable/disable vector retrieval in this phase.
    - `vector_auto`: enable/disable auto-adaptation gate.
    - `vector_min_query_chars`: per-phase query length threshold.
    - `vector_top_k`: per-phase top-k retrieval count.
    - `vector_min_similarity`: per-phase similarity threshold.
    - `vector_max_snippet_chars`: per-phase snippet cap.
    - `summary_enabled`: enable/disable summary memory in this phase.
    - `summary_trigger_messages`: per-phase summary trigger threshold.
    - `summary_max_chars`: per-phase summary cap.
  - Token optimization options (optional):
    - `max_history_messages`: cap history message count before model call.
    - `max_history_chars`: cap total history characters before model call.
  - Timeout options (optional):
    - `request_timeout_seconds`: hard limit for the main phase agent execution.
    - `review_timeout_seconds`: hard limit for each review-gate agent; falls back to `request_timeout_seconds` when omitted.
  - Full-auto review options (optional):
    - `full_auto_review_agents`: explicit reviewer agent list. In complex mode, the first 2 are the fixed review gate.
    - `min_reviewers`: minimum reviewer pool size required before complex full_auto can run.
    - `required_approvals`: approvals required to pass the review gate (must be <= `min_reviewers`).
    - `review_gate_timeout_seconds`: hard deadline for the full review gate loop.
    - `review_timeout_policy`: timeout behavior when gate deadline is reached.
      - `reject` (default): reject execution when gate times out.
      - `degrade_single`: allow execution if at least one reviewer already approved before timeout.
  - Runtime protection options in `options.extra` (optional):
    - `max_request_chars`: hard cap on total message characters per request.
    - `rate_limit_rpm`: per-phase request rate limit (requests/minute).
    - `rate_limit_burst`: token bucket capacity for short spikes.
    - `rate_limit_burst_multiplier`: alternative burst capacity as `rate_limit_rpm * multiplier`.
    - `phase_max_inflight`: max concurrent in-flight requests for the phase.
    - `global_max_inflight`: max concurrent in-flight requests process-wide.
    - `circuit_breaker_failures`: consecutive failures before opening breaker.
    - `circuit_breaker_open_seconds`: breaker open duration before retry.
    - If fewer than 2 review-capable AIs are configured, complex mode should not be used.

Agent type notes:

- `copilot`: local service URL (`url`) and no API key requirement by default.
- `deepseek`: requires `api_key_env` and `model` (default in example: `deepseek-chat`).
- `wenxin`: requires `api_key_env` and `secret_key_env`.
- For all `*_env` secret fields above, values can be either:
  - environment variable names (legacy behavior), or
  - keyring references in form `keyring://<service>/<account>`.
- `openai_compatible`: generic provider adapter requiring:
  - `url`: base URL, for example `https://api.openai.com`
  - `chat_path` (optional): path to chat endpoint (default `/v1/chat/completions`)
  - `api_key_env`: env var storing bearer token
  - `model`: model name
  - `supports_system` (optional, default `true`):
    - `true`: principles are injected as a `system` message
    - `false`: principles are prefixed into first `user` message
- `doubao`: explicit provider type (independent from generic type) requiring:
  - `url`: base URL, for example `https://ark.cn-beijing.volces.com/api/v3`
  - `chat_path` (optional): default `/chat/completions`
  - `api_key_env`: env var for Doubao/Volcengine token
  - `model`: Doubao model id
  - `supports_system` (optional, default `true`)
- `claude`: explicit provider type (Anthropic Messages API) requiring:
  - `api_key_env`: env var for Anthropic API key
  - `model`: Claude model name
  - `url` (optional): default `https://api.anthropic.com`
  - `anthropic_version` (optional): default `2023-06-01`
  - `max_tokens` (optional): default `4096`

## Zed settings.json Example

Add this to your Zed `settings.json` `agent_servers` section:

```json
{
  "agent_servers": {
    "go-on": {
      "command": "/absolute/path/to/go-on",
      "args": [
        "--config",
        "/absolute/path/to/config.toml"
      ],
      "env": {
        "DEEPSEEK_API_KEY": "${DEEPSEEK_API_KEY}",
        "WENXIN_API_KEY": "${WENXIN_API_KEY}",
        "WENXIN_SECRET_KEY": "${WENXIN_SECRET_KEY}"
      }
    }
  }
}
```

## Notes About Principles and Fallback

- Principles are phase-specific and merged into each model request automatically.
- Copilot receives principles as a prefixed user instruction.
- DeepSeek and Wenxin receive principles as `system` content.
- If phase `fallback = true`, the proxy tries each configured agent in order until one succeeds.
- If phase `fallback = false`, only the first configured/available agent is used.

## Notes About Timeouts

- Phase execution can now be bounded with `request_timeout_seconds`.
- Review-gate agents can use a stricter `review_timeout_seconds` override.
- A timed out agent is aborted and counted as a failed candidate.

## Notes About Cache

- Cache key is generated from normalized request context: phase, optimized messages,
  principles, options, and candidate agent list.
- On cache hit, proxy streams the cached response immediately and returns `cached: true`.
- Only successful non-empty responses are written into cache.

## Notes About Vector Retrieval

- Vector retrieval is attempted only when enabled and query complexity crosses thresholds.
- In auto mode, short/simple prompts skip vector lookup to avoid unnecessary overhead.
- Retrieved snippets are injected as a compact reference block before model invocation.
- Successful responses are persisted into vector memory for future similarity retrieval.

## Notes About Summary Memory

- Summary memory is phase-scoped and persisted in SQLite.
- For long sessions, summary is injected as compact context before model invocation.
- Summary updates happen after successful responses and are size-capped.

## Runtime Metrics

- `metrics.get` JSON-RPC method returns counters for:
  - chat requests total
  - cache lookup/hit/store
  - vector search/hit/store
  - summary read/hit/store
  - agent failures total
  - review gate totals, approval/rejection counts, timeout counts, degraded approvals, and invalid reviewer responses
- `metrics.reset` clears all counters.
- `metrics.prometheus` exports Prometheus text exposition with `# HELP` / `# TYPE` metadata.
- `metrics.prometheus` now also includes latency histograms:
  - `acp_chat_latency_seconds`
  - `acp_agent_latency_seconds`
  - `acp_review_latency_seconds`
- `metrics.prometheus` now includes failure buckets:
  - `acp_agent_timeout_failures_total`
  - `acp_agent_panic_failures_total`
  - `acp_agent_other_failures_total`
- `metrics.prometheus` also includes review gate timeout and degrade counters:
  - `acp_review_gate_timeout_total`
  - `acp_review_gate_degraded_total`
  - `acp_review_gate_invalid_response_total`
- `metrics.prometheus` also includes labeled runtime gauges for:
  - in-flight requests by global/phase scope
  - circuit breaker state per agent
  - token bucket status per phase
  - lifecycle shutdown state and maintenance activity
- `initialize` capabilities now include `metrics: true`.

## Runtime Health And Maintenance RPCs

- `runtime.health` returns runtime health snapshot:
  - in-memory cache entries
  - sqlite cache entry count (if enabled)
  - circuit breaker open/half-open/tracked agents
  - rate limiter tracked phases
  - vector memory and summary entry counts (if enabled)
  - lifecycle state and shutdown reason
  - latest maintenance cycle status
  - review gate totals including timeout/degraded/invalid-response counters
- Background maintenance runs on the `[runtime].maintenance_interval_seconds` cadence and reuses the same GC path as `maintenance.gc`.
- Background health logging runs on the `[runtime].health_interval_seconds` cadence.
- `maintenance.gc` purges expired cache records:
  - in-memory L1 expired entries
  - sqlite expired rows
- `cache.clear` clears response cache data:
  - in-memory L1 cache
  - sqlite response cache rows
- `vector.clear` clears vector memory and stored phase summaries.
- `breaker.status` returns per-agent breaker state, including `closed`, `open`, `half_open`, and `half_open_ready`.
- `breaker.reset` clears breaker state for one agent (with `{"agent":"name"}`) or all agents (without params).
- `phase.status` returns per-phase token bucket and in-flight status snapshots.
- `config.reload` reloads config file, validates env requirements and keyring references, returns collected warnings plus `health` score details, and hot-swaps flow/agent/cache/vector/autotune/runtime resources.
- `shutdown` marks the server as draining, rejects new chat work, and waits up to `[runtime].shutdown_drain_seconds` for in-flight requests to finish.

## Request Templates

The repository includes ready-to-send NDJSON examples under `requests/` for runtime checks, reload flows, breaker resets, and graceful shutdown sequences.

Use helper scripts to execute templates quickly:

- Windows PowerShell:
  - `./scripts/run-request.ps1 -Config ./config.toml -Template ./requests/runtime-health.ndjson`
- Bash:
  - `./scripts/run-request.sh ./config.toml ./requests/runtime-health.ndjson`

## Autotune Status

- `[autotune]` values are parsed, validated, and loaded into runtime state.
- `autotune.get` returns the persisted runtime tuning snapshot.
- `autotune.reset` restores runtime tuning state to config defaults and persists it.
- `initialize.capabilities.autotune` reports whether autotune is enabled.
- Runtime tuning currently feeds live vector-search `min_query_chars` and `top_k` defaults based on precision feedback and cooldown windows.

## Chat Modes (Zed)

- Pass `mode` in `chat` request params:
  - `ask`: defaults to `review` phase when `phase` is not provided, otherwise falls back to `default_phase` if no `review` phase exists.
  - `edit`: defaults to `coding` phase when `phase` is not provided.
  - `agent`: defaults to `coding` phase when `phase` is not provided.
  - `full_auto`: defaults to `coding` phase and runs dual review gate first.
- `full_auto` approval gate behavior:
  - Two reviewers must both return `APPROVE`.
  - If either reviewer rejects, request is stopped and no execution is performed.
  - If `review_gate_timeout_seconds` is hit, behavior follows `review_timeout_policy`.
  - Cache is bypassed in `full_auto` mode to ensure fresh approval.

