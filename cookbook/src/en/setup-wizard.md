# Setup Wizard

The setup wizard is implemented in the backend and is the recommended way to bootstrap a clean machine or a fresh working directory.

## What it writes

The setup flow targets the backend configuration path and then writes supporting defaults around it:

- `config.toml`
- default rules files near the config directory
- optional secret references for environment variables or keyring-backed values

The setup path currently uses the adaptive template family.

## Entry points

Interactive mode:

```bash
go-on --setup
```

Non-interactive leaning mode:

```bash
go-on --setup --setup-profile adaptive --setup-level standard --setup-secrets auto
```

Overwrite existing files:

```bash
go-on --setup --force
```

## Accepted setup profile

Current accepted profile:

- `adaptive`

That matches the runtime architecture: one config baseline that can serve ACP and MCP oriented front ends.

## Setup levels

### `quick`

Use this when you want the shortest successful bootstrap.

Behavioral characteristic:

- skips the extra-agent prompt to keep the flow minimal

### `standard`

Recommended default.

Use this when you want a balanced guided setup without having to hand-edit everything afterward.

### `custom`

Use this when you want more manual control over provider and agent choices.

## Secret modes

### `env`

Keep secrets as environment-driven values.

Use this when you already manage keys through shell profile, `.env`, CI, or process manager injection.

### `keyring`

Store secret material in the OS keyring and let config refer to keyring-backed entries.

Use this when you want a local desktop-friendly secret flow with less plain-text exposure.

### `auto`

Auto-detect the best path.

Implementation behavior:

- if environment variables are already available, setup prefers `env`
- otherwise the wizard prompts for secret-handling choice

## Provider detection behavior

The wizard detects available providers based on the chosen secret mode and the current machine state.

The flow then:

1. detects providers
2. prompts for provider selection
3. applies setup-level defaults
4. writes the adaptive config
5. optionally stores secrets in keyring

If no provider is selected, setup aborts rather than generating a misleading non-runnable config.

## Keyring behavior

When keyring mode is selected, the generated config is converted from environment placeholders to keyring-backed references.

In this repository those references follow the `keyring://go-on/<account>` pattern.

## Secret management outside the wizard

The wizard is not the only secret entry point. You can manage secrets later with CLI actions:

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
```

That is the clean recovery path if setup was correct but credentials changed later.

## Recommended setup sequence

For most operators:

1. Run `go-on --setup --setup-level standard --setup-secrets auto`.
2. Run `go-on --status`.
3. If you will use Zed or GUI over HTTP, start the backend with `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090`.
4. If you will use an editor-spawned stdio integration, keep `adaptive` or switch that client to `acp_stdio` or `mcp_stdio` when you need a fixed protocol surface.

## When to rerun setup

Rerun setup when:

- moving to a new machine
- replacing your provider set
- switching from env-based secrets to keyring-based secrets
- restoring a broken or missing `config.toml`

Avoid rerunning setup with `--force` unless you intend to replace the current file set.