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
Usage: go-on.exe [OPTIONS]
```

There are no subcommands. Everything is driven by flags.

## Core runtime options

### `--config <CONFIG>`

Use an explicit configuration file path. If omitted, the runtime resolves `config.toml` from the executable directory.

Example:

```bash
go-on --config D:\go-on\config.toml
```

### `--phase <PHASE>`

Select a specific phase profile to run. Use this when your config defines multiple phase-oriented behaviors and you want one deterministic entry point.

### `--verbose`

Enable verbose logging. Use this first when diagnosing startup, config, transport, or provider readiness issues.

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

- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

Recommended usage:

- `adaptive`: safest default when multiple clients may attach.
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

## Common command recipes

Minimal setup:

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
go-on --config config.toml --protocol-mode acp_stdio --verbose
```

## Operational guidance

- Use `--validate-config` before assuming the transport layer is broken.
- Use `--status` before opening the GUI or an editor plugin.
- Use `adaptive` unless you have a concrete client contract that requires ACP-only or MCP-only behavior.
- Prefer `--add-local-model` over hand-editing config when onboarding a local OpenAI-compatible endpoint.